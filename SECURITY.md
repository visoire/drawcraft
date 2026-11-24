# Security Policy

## Principles
1. **Local-first, zero telemetry:** notebooks, PDFs and indexes stay on the user's device. DrawCraft has no analytics, tracking or silent uploads.
2. **Hostile input:** every PDF, `.dcraft` / `.dcraftpkg` file, backup archive, `.docx`, image, pointer stream and JSON command is untrusted. No panics (`unwrap` / `expect` / `panic!` are denied in non-test code), bounded allocations and recursion, `NaN` / `Infinity` sanitized, archive entry counts and sizes capped.
3. **No credentials in the browser:** the web app never stores third-party provider secrets or backend admin credentials.
4. **Atomic persistence:** saves write changed pages first and the manifest last; unreadable files are quarantined as `.corrupt`, never silently overwritten.

## Reporting a vulnerability
Please do not open public issues for security problems. Use the repository's private security advisory (Security tab, "Report a vulnerability") with reproduction steps, affected version and impact.