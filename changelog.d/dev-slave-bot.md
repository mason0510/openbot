### Changed

- The desktop app name is slavebot. A packaged build signs in to the Attendant account API at `https://api.attendant.mwcpool.uk`.
- A hosted server install accepts Ubuntu 22.04. The installer skips the AppArmor `abi/4.0` profile on that release.
- The Attendant account API source is in `attendant/`. It serves `POST /v1/mobile-auth/ticket` and `POST /v1/mobile-auth/redeem`.
