# Release signing (maintainer-only, local)

Windows release binaries are signed locally with the maintainer's SSL.com EV code-signing
certificate on a YubiKey. CI never sees key material; it builds unsigned artifacts, the
maintainer signs the Windows exe and uploads the signed file plus fresh SHA-256 sums.

## Procedure

1. Insert the YubiKey; confirm the cert is visible: `certutil -user -store My`
   (note the certificate thumbprint).
2. Sign (Windows SDK signtool; SafeNet/YubiKey CSP will prompt for the PIN):

   ```
   signtool sign /fd sha256 /tr http://ts.ssl.com /td sha256 /sha1 <THUMBPRINT> OpenNtripClient.exe
   ```

3. Verify:

   ```
   signtool verify /pa /v OpenNtripClient.exe
   ```

4. Recompute checksums and update the release notes:

   ```
   certutil -hashfile OpenNtripClient.exe SHA256
   ```

Notes:

- `/tr http://ts.ssl.com` is SSL.com's RFC 3161 timestamp service; timestamping is
  mandatory so signatures outlive certificate expiry.
- EV signatures establish SmartScreen reputation immediately; unsigned dev builds will
  warn - that is expected and documented in the README.
