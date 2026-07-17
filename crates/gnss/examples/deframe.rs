//! Offline RTCM capture inspector: the file-based half of live stream
//! verification. Point it at a raw RTCM3 capture (e.g. bytes recorded off a
//! caster) and it reports what a healthy session should show: per-type
//! counts, integrity counters, and one decoded sample of every message type
//! we can explain.
//!
//! Usage: cargo run -p gnss --example deframe -- <capture.rtcm3>

use gnss::rtcm::decode::{Decoded, decode, type_name};
use gnss::rtcm::frame::{Deframer, FrameEvent};
use std::collections::BTreeMap;

struct Stat {
    count: u64,
    bytes: u64,
}

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: deframe <capture.rtcm3>");
        std::process::exit(2);
    };
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            std::process::exit(1);
        }
    };

    let mut deframer = Deframer::new();
    let mut stats: BTreeMap<u16, Stat> = BTreeMap::new();
    let mut samples: BTreeMap<u16, Decoded> = BTreeMap::new();

    // Chunked feeding exercises the same resume-across-reads path a live
    // TCP stream does.
    for chunk in data.chunks(4096) {
        deframer.feed(chunk, &mut |event| {
            if let FrameEvent::Frame { msg_type, payload } = event {
                let s = stats.entry(msg_type).or_insert(Stat { count: 0, bytes: 0 });
                s.count += 1;
                s.bytes += payload.len() as u64 + 6; // header + CRC included
                if !samples.contains_key(&msg_type)
                    && let Some(d) = decode(msg_type, payload)
                {
                    samples.insert(msg_type, d);
                }
            }
        });
    }

    println!("capture: {path} ({} bytes)", data.len());
    println!();
    println!(
        "{:>6}  {:<38} {:>8} {:>12}",
        "type", "name", "count", "bytes"
    );
    println!("{:->6}  {:-<38} {:->8} {:->12}", "", "", "", "");
    let mut total_frames = 0u64;
    let mut total_bytes = 0u64;
    for (t, s) in &stats {
        let name = type_name(*t).unwrap_or("-");
        println!("{t:>6}  {name:<38} {:>8} {:>12}", s.count, s.bytes);
        total_frames += s.count;
        total_bytes += s.bytes;
    }
    println!("{:->6}  {:-<38} {:->8} {:->12}", "", "", "", "");
    println!(
        "{:>6}  {:<38} {total_frames:>8} {total_bytes:>12}",
        "", "total"
    );
    println!();
    println!("crc failures : {}", deframer.crc_failures);
    println!("garbage bytes: {}", deframer.garbage_bytes);

    if !samples.is_empty() {
        println!();
        println!("decoded samples (first of each type):");
        for (t, d) in &samples {
            println!();
            print_decoded(*t, d);
        }
    }
}

fn print_decoded(msg_type: u16, d: &Decoded) {
    match d {
        Decoded::BasePosition {
            station_id,
            is_1006,
            ecef_x_m,
            ecef_y_m,
            ecef_z_m,
            antenna_height_m,
            lla,
        } => {
            let kind = if *is_1006 { "1006" } else { "1005" };
            println!("[{kind}] base position, station {station_id}");
            println!("  ECEF  x {ecef_x_m:.4} m, y {ecef_y_m:.4} m, z {ecef_z_m:.4} m");
            match lla {
                Some((lat, lon, alt)) => {
                    println!("  LLA   {lat:.8} deg, {lon:.8} deg, {alt:.3} m");
                }
                None => println!("  LLA   position not set (all-zero ECEF)"),
            }
            if let Some(h) = antenna_height_m {
                println!("  antenna height {h:.4} m");
            }
        }
        Decoded::AntennaInfo {
            station_id,
            antenna,
            setup_id,
            antenna_serial,
            receiver,
            firmware,
            receiver_serial,
        } => {
            println!("[{msg_type}] antenna/receiver info, station {station_id}");
            println!("  antenna  {antenna:?} (setup {setup_id})");
            if let Some(s) = antenna_serial {
                println!("  antenna serial {s:?}");
            }
            if let Some(s) = receiver {
                println!("  receiver {s:?}");
            }
            if let Some(s) = firmware {
                println!("  firmware {s:?}");
            }
            if let Some(s) = receiver_serial {
                println!("  receiver serial {s:?}");
            }
        }
        Decoded::TextMessage {
            station_id,
            mjd,
            seconds_of_day,
            text,
        } => {
            println!("[1029] text message, station {station_id} (MJD {mjd}, {seconds_of_day} s)");
            println!("  {text:?}");
        }
        Decoded::GlonassBiases {
            station_id,
            bias_indicator,
            biases_m,
        } => {
            println!("[1230] GLONASS code-phase biases, station {station_id}");
            println!("  aligned: {bias_indicator}");
            const SIGNALS: [&str; 4] = ["L1 C/A", "L1 P", "L2 C/A", "L2 P"];
            for (signal, meters) in biases_m {
                let name = SIGNALS.get(usize::from(*signal)).copied().unwrap_or("?");
                println!("  {name}: {meters:.2} m");
            }
        }
        Decoded::MsmHeader {
            constellation,
            msg_type,
            station_id,
            epoch,
            num_sats,
            num_signals,
            signal_ids,
        } => {
            println!("[{msg_type}] {constellation} MSM, station {station_id}");
            println!(
                "  epoch {epoch}, {num_sats} satellites x {num_signals} signals, signal ids {signal_ids:?}"
            );
        }
    }
}
