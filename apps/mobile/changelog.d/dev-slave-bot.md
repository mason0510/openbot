### Changed

- Phone email sign-in and registration use `https://api-attendant.mwcpool.uk`. A stored session for `https://api.attendant.mwcpool.uk` moves to that host. They do not use `https://api.openbot.run`.
- The phone restores a host public key whose newlines were stored as the letter n. After Signal `ready`, it replaces a non-empty STUN list with this computer's IPv6 STUN address.
