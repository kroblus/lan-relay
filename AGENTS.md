# LAN Relay / 随手传

Personal LAN app. Node.js 24+ built-ins; no runtime npm dependencies. Browser UI is native HTML/CSS/ES modules. Data stays on the host PC. Do not deploy to a public host or enable internet forwarding as part of routine development.

## Work boundaries
- Keep source, tests and documentation in this repository. Generated test evidence belongs in ignored `output/verification/`.
- Never commit user files, runtime databases, owner bootstrap secrets, pairing tokens, certificates or cookies.
- Follow `docs/API.md` for the V1 contract; coordinate contract changes before touching clients.
- Authentication and upload correctness are material: test pairing, CSRF/origin checks, revoked devices, interrupted transfer, integrity and Range download.
- Never equate a download attempt with saved, installed or verified. Installation/test statuses are explicit user confirmations in V1.
- HTTP LAN mode does not offer encryption or full PWA capabilities. Do not weaken browser or OS protections.
- No arbitrary shell execution, whole-disk browsing, automatic APK installation or clearing app data.
- Prefer focused tests for observable behavior. No invented APK metadata, device facts, screenshots or physical-device PASS claims.
- APK metadata defaults to the bounded built-in ZIP/AXML reader. An explicitly configured SDK executable is only a fallback when the built-in status is not `available`; missing fields and resource references remain unknown. Metadata parsing does not validate signatures or installability.
- PWA installation, system share targets and background clipboard sync are outside implemented V1, including under HTTPS. Keep Windows CI, browser simulations, Windows local startup and physical Android results distinct.
- Do not change unrelated projects or push fixes without the current user's authorization.

## Commands
`npm start` runs the app; `npm test` runs the meaningful Node tests; `npm run check` checks syntax and tracked secret exclusions.
