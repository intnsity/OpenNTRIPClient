//! TLS transport for the NTRIP worker: rustls client configs (verified via
//! the webpki root store, or the loud diagnostic accept-anything override)
//! and a cancellable blocking handshake over an already-connected socket.
//!
//! The worker keeps holding the RAW TcpStream clone for cancellation:
//! `shutdown()` on that clone makes the reads rustls performs underneath
//! `StreamOwned` fail immediately, so the existing cancel path needs no TLS
//! awareness.

use std::io;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

/// Negotiated-session facts for the connection log: every handshake outcome
/// is a support artifact ("was this really TLS 1.2? which suite?").
pub struct TlsInfo {
    pub protocol: String,
    pub cipher: String,
}

/// Handshake budget, mirroring ntrip-core's FIRST_RESPONSE_TIMEOUT: a server
/// that accepted the TCP connection but never speaks TLS (plain-NTRIP port,
/// accept-and-blackhole firewall) must fail the same way the plain-TCP path
/// does, so the reconnect supervisor gets a chance to ride the outage instead
/// of sitting in "Connecting" until a human clicks Disconnect.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Handshake failure, classified for the reconnect supervisor.
pub enum TlsFail {
    /// The user cancelled while the handshake was in flight.
    Cancelled,
    /// TLS-level rejection (certificate verification, protocol mismatch).
    /// Permanent: retrying yields the identical failure forever.
    Tls(String),
    /// Transport-level failure (reset, EOF mid-handshake). Environment-shaped
    /// and worth a reconnect, same as any other connection drop.
    Io(String),
}

/// Complete a TLS handshake on `sock` (which must already carry the worker's
/// 400 ms read timeout - it doubles as the cancel-and-deadline poll cadence
/// here) and return the wrapped stream plus the negotiated-session facts.
/// `timeout` bounds the whole handshake; expiry is a transport-shaped
/// `TlsFail::Io`, not a TLS rejection, because a silent peer is environment
/// trouble a reconnect can ride out.
pub fn handshake(
    mut sock: TcpStream,
    host: &str,
    allow_invalid: bool,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<(Box<StreamOwned<ClientConnection, TcpStream>>, TlsInfo), TlsFail> {
    let deadline = Instant::now() + timeout;
    // ServerName parses IP literals into the IpAddress variant, which is how
    // bare-IP casters (whose certs carry IP SANs, or no valid SAN at all -
    // that is what the diagnostic override is for) stay reachable.
    let name = ServerName::try_from(host.to_string())
        .map_err(|e| TlsFail::Tls(format!("invalid TLS server name '{host}': {e}")))?;
    let mut conn = ClientConnection::new(client_config(allow_invalid), name)
        .map_err(|e| TlsFail::Tls(e.to_string()))?;
    while conn.is_handshaking() {
        if cancel.load(Ordering::SeqCst) {
            return Err(TlsFail::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(TlsFail::Io("TLS handshake timed out".to_string()));
        }
        match conn.complete_io(&mut sock) {
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(e) => {
                if cancel.load(Ordering::SeqCst) {
                    return Err(TlsFail::Cancelled);
                }
                // rustls surfaces its own alerts/verification failures as
                // InvalidData wrapping a rustls::Error; everything else is
                // plain transport trouble.
                return Err(if e.kind() == io::ErrorKind::InvalidData {
                    TlsFail::Tls(e.to_string())
                } else {
                    TlsFail::Io(e.to_string())
                });
            }
        }
    }
    let info = TlsInfo {
        protocol: conn
            .protocol_version()
            .map_or_else(|| "unknown version".to_string(), |v| format!("{v:?}")),
        cipher: conn.negotiated_cipher_suite().map_or_else(
            || "unknown cipher".to_string(),
            |c| format!("{:?}", c.suite()),
        ),
    };
    Ok((Box::new(StreamOwned::new(conn, sock)), info))
}

/// The two client configs the app ever needs, built once. Config
/// construction parses the entire webpki root store; per-connection rebuilds
/// would be pure waste on a reconnect ladder.
fn client_config(allow_invalid: bool) -> Arc<ClientConfig> {
    static VERIFIED: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    static INSECURE: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    if allow_invalid {
        INSECURE
            .get_or_init(|| {
                Arc::new(
                    ClientConfig::builder()
                        .dangerous()
                        .with_custom_certificate_verifier(Arc::new(NoVerify::new()))
                        .with_no_client_auth(),
                )
            })
            .clone()
    } else {
        VERIFIED
            .get_or_init(|| {
                let mut roots = rustls::RootCertStore::empty();
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                Arc::new(
                    ClientConfig::builder()
                        .with_root_certificates(roots)
                        .with_no_client_auth(),
                )
            })
            .clone()
    }
}

/// The diagnostic override: accepts ANY certificate and signature. Gated
/// behind the profile's `allow_invalid_certs` flag, and the UI shows a
/// persistent red banner whenever a connection runs through this verifier -
/// it exists so support staff can reach casters with self-signed or bare-IP
/// certificates, not to make failures quiet.
#[derive(Debug)]
struct NoVerify {
    schemes: Vec<SignatureScheme>,
}

impl NoVerify {
    fn new() -> Self {
        NoVerify {
            // Advertise everything the crypto provider could verify, so the
            // server never aborts for lack of a mutually supported scheme.
            schemes: rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes(),
        }
    }
}

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configs_are_cached_and_distinct() {
        let a = client_config(false);
        let b = client_config(false);
        assert!(Arc::ptr_eq(&a, &b), "verified config built once");
        let c = client_config(true);
        let d = client_config(true);
        assert!(Arc::ptr_eq(&c, &d), "insecure config built once");
        assert!(!Arc::ptr_eq(&a, &c));
    }

    #[test]
    fn server_name_accepts_dns_and_ip() {
        assert!(ServerName::try_from("caster.example.com".to_string()).is_ok());
        // TEST-NET-1 documentation address: any IP literal proves the parse.
        let ip = ServerName::try_from("192.0.2.1".to_string()).unwrap();
        assert!(matches!(ip, ServerName::IpAddress(_)));
        assert!(ServerName::try_from("bad host name".to_string()).is_err());
    }

    #[test]
    fn no_verify_advertises_schemes() {
        assert!(!NoVerify::new().schemes.is_empty());
    }

    /// The handshake deadline: a server that accepts the TCP connection and
    /// then never sends a byte must fail as Io within the budget, so the
    /// reconnect supervisor can classify it as environment-shaped. Before the
    /// deadline existed this looped on read timeouts forever.
    #[test]
    fn handshake_times_out_against_silent_server() {
        use std::io::Read;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            // Accept, then read forever and never write: the client's
            // ClientHello lands here and gets no ServerHello back.
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            loop {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });

        let sock = TcpStream::connect(addr).unwrap();
        // Short read timeout = fast poll cadence; short budget keeps the
        // test quick. The production caller passes HANDSHAKE_TIMEOUT.
        sock.set_read_timeout(Some(Duration::from_millis(25)))
            .unwrap();
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let result = handshake(
            sock,
            "silent.example.com",
            false,
            &cancel,
            Duration::from_millis(300),
        );
        let elapsed = started.elapsed();
        match result {
            Err(TlsFail::Io(msg)) => assert!(msg.contains("timed out"), "{msg}"),
            Err(TlsFail::Tls(msg)) => panic!("classified permanent: {msg}"),
            Err(TlsFail::Cancelled) => panic!("classified cancelled"),
            Ok(_) => panic!("handshake cannot succeed against a silent peer"),
        }
        assert!(
            elapsed < Duration::from_secs(5),
            "deadline not honored: {elapsed:?}"
        );
        server.join().unwrap();
    }
}
