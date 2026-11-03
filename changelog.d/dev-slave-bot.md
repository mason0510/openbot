### Changed

- The desktop app name is slavebot. A packaged build signs in to the Attendant account API at `https://api-attendant.mwcpool.uk`.
- At startup the main process reads the machine proxy. When one is enabled, account HTTP and the Signal `wss` socket use one `http://host:port` hop, and HTTP/2 is off so Signal can send a classic upgrade. An HTTP hop wins over SOCKS on the same port. No machine proxy leaves Chromium direct and does not read `HTTP_PROXY`.
- The packaged app allows the peer window to open `wss://api-attendant.mwcpool.uk`. The page policy otherwise allows only `wss://*.openbot.run`.
- A hosted server install accepts Ubuntu 22.04. The installer skips the AppArmor `abi/4.0` profile on that release.
- The Attendant account API source is in `attendant/`. It serves email sign-in, host registration, mobile tickets, in-memory remote sessions, and Signal tickets. A client Signal ticket is an ES256 JWT. `GET /v2/hosting/servers/{id}/wake` stays 404 because this computer is not a cloud host.
- The desktop disables mDNS hiding so the phone can see the LAN address. The phone restores a host public key whose newlines were stored as the letter n, and replaces a non-empty ready STUN list with this computer's IPv6 STUN address.
