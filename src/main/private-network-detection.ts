import { networkInterfaces } from "node:os";

// Tailscale gives each device an address in this unique local IPv6 prefix. Other VPNs, such as
// Cloudflare WARP, also use 100.64.0.0/10 for IPv4, so only this prefix identifies Tailscale.
const TAILSCALE_IPV6_PREFIX = "fd7a:115c:a1e0:";

/**
 * Whether Tailscale is connected on this computer. Reads only the network interfaces: running the
 * Tailscale CLI can show a permission prompt for the Mac App Store version.
 */
export function isTailscaleConnected(): boolean {
  return Object.values(networkInterfaces()).some((addresses) =>
    (addresses ?? []).some(
      (address) => address.family === "IPv6" && address.address.toLowerCase().startsWith(TAILSCALE_IPV6_PREFIX),
    ),
  );
}

/**
 * One LAN IPv4 and one global IPv6 from the interfaces at this call.
 * Guarantees loopback, link-local, and 169.254 are absent. Prefers 192.168/10/172.16 over other
 * IPv4, and a non-ULA IPv6 over fc/fd, so a VPN address does not hide the house LAN.
 * Does not invent an address. A missing family is omitted. Does not open a socket.
 */
export function hostInterfaceAddresses(): string[] {
  let v4 = "";
  let v4Rank = 9;
  let v6 = "";
  let v6Ula = true;
  for (const list of Object.values(networkInterfaces())) {
    for (const item of list ?? []) {
      if (item.internal) continue;
      const address = item.address.split("%")[0] ?? "";
      if (!address) continue;
      if (item.family === "IPv4") {
        const rank = ipv4Rank(address);
        if (rank < v4Rank) {
          v4 = address;
          v4Rank = rank;
        }
      } else if (item.family === "IPv6") {
        const lower = address.toLowerCase();
        if (lower.startsWith("fe80:") || lower === "::" || lower === "::1") continue;
        const ula = lower.startsWith("fc") || lower.startsWith("fd");
        if (!v6 || (v6Ula && !ula)) {
          v6 = lower;
          v6Ula = ula;
        }
      }
    }
  }
  return [v4, v6].filter((address) => address.length > 0);
}

/** Lower is better. 9 means drop. 100.64/10 is CGNAT, behind RFC1918. */
function ipv4Rank(address: string): number {
  if (address === "0.0.0.0" || address.startsWith("127.") || address.startsWith("169.254.")) return 9;
  if (address.startsWith("192.168.")) return 0;
  if (address.startsWith("10.")) return 1;
  const parts = address.split(".");
  const first = Number(parts[0]);
  const second = Number(parts[1]);
  if (first === 172 && second >= 16 && second <= 31) return 1;
  if (first === 100 && second >= 64 && second <= 127) return 3;
  return 2;
}
