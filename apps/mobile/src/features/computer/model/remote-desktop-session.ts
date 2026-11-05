import {
  REMOTE_DESKTOP_ERROR_CODES,
  type RemoteDesktopCapabilities,
  type RemoteDesktopSession,
} from "@openbot/contracts/ipc";
import { isBoolean, isDynamicRecord, isNumber, isOneOf, isString } from "@openbot/contracts/runtime-values";
import { TEAM_API_ROUTES } from "@openbot/contracts/team-api-routes";

/** Host advertises remote desktop for Sunshine/Moonlight screen share. */
export const REMOTE_DESKTOP_CAPABILITY = "remote-desktop";

export function supportsRemoteDesktop(capabilities: readonly string[] | undefined): boolean {
  return Boolean(capabilities?.includes(REMOTE_DESKTOP_CAPABILITY));
}

function decodeDisplays(value: unknown): RemoteDesktopCapabilities["displays"] {
  if (!Array.isArray(value)) throw new Error("Invalid remote display list.");
  return value.map((item) => {
    if (
      !isDynamicRecord(item) ||
      !isString(item.id) ||
      !isString(item.label) ||
      !isNumber(item.width) ||
      !isNumber(item.height) ||
      !isBoolean(item.primary)
    ) {
      throw new Error("Invalid remote display.");
    }
    return {
      id: item.id,
      label: item.label,
      width: item.width,
      height: item.height,
      primary: item.primary,
    };
  });
}

export function decodeRemoteDesktopCapabilities(value: unknown): RemoteDesktopCapabilities {
  if (!isDynamicRecord(value)) throw new Error("Invalid remote control capabilities.");
  const platform = value.platform;
  if (!isOneOf(["darwin", "win32", "linux"] as const, platform)) throw new Error("Invalid remote platform.");
  if (!isBoolean(value.ready) || !isBoolean(value.unattended)) throw new Error("Invalid remote control capabilities.");
  if (value.runtime !== "sunshine-moonlight") throw new Error("Invalid remote desktop runtime.");
  if (value.protocolVersion !== 2) throw new Error("Remote desktop update required.");
  if (!isNumber(value.activeSessions) || !isNumber(value.maxSessions)) {
    throw new Error("Invalid remote control capabilities.");
  }
  return {
    ready: value.ready,
    platform,
    unattended: value.unattended,
    runtime: "sunshine-moonlight",
    protocolVersion: 2,
    displays: decodeDisplays(value.displays),
    selectedDisplayId:
      value.selectedDisplayId === null || isString(value.selectedDisplayId) ? value.selectedDisplayId : null,
    activeSessions: value.activeSessions,
    maxSessions: value.maxSessions,
  };
}

function invalidRemoteError(): never {
  throw new Error("Invalid remote control error.");
}

export function decodeRemoteDesktopSession(value: unknown): RemoteDesktopSession {
  if (!isDynamicRecord(value)) throw new Error("Invalid remote control session.");
  const phase = value.phase;
  const transport = value.transport;
  if (!isOneOf(["starting_host", "connecting", "connected", "disconnecting", "error"] as const, phase)) {
    throw new Error("Invalid remote control phase.");
  }
  if (!isOneOf(["unknown", "p2p", "relay"] as const, transport)) throw new Error("Invalid remote transport.");
  const errorCode =
    value.errorCode === null || value.errorCode === undefined
      ? null
      : isOneOf(REMOTE_DESKTOP_ERROR_CODES, value.errorCode)
        ? value.errorCode
        : invalidRemoteError();
  if (
    !isString(value.id) ||
    !isString(value.serverId) ||
    !isString(value.viewerUrl) ||
    !isString(value.viewerGrant) ||
    !isString(value.createdAt) ||
    !isString(value.grantExpiresAt)
  ) {
    throw new Error("Invalid remote control session.");
  }
  return {
    id: value.id,
    serverId: value.serverId,
    viewerUrl: value.viewerUrl,
    viewerGrant: value.viewerGrant,
    displays: decodeDisplays(value.displays),
    selectedDisplayId:
      value.selectedDisplayId === null || isString(value.selectedDisplayId) ? value.selectedDisplayId : null,
    phase,
    transport,
    errorCode,
    message: value.message === null || isString(value.message) ? value.message : null,
    createdAt: value.createdAt,
    grantExpiresAt: value.grantExpiresAt,
  };
}

export function ignoreRemoteDesktopResponse(_value: unknown): null {
  return null;
}

export const remoteDesktopRoutes = {
  capabilities: TEAM_API_ROUTES.remoteScreen.capabilities,
  sessions: TEAM_API_ROUTES.remoteScreen.sessions,
  session: TEAM_API_ROUTES.remoteScreen.session,
  display: TEAM_API_ROUTES.remoteScreen.display,
  authorize: (sessionId: string) => `${TEAM_API_ROUTES.remoteScreen.session(sessionId)}/authorize`,
  viewer: TEAM_API_ROUTES.remoteScreen.viewer,
  streamPath: (sessionId: string) => `${TEAM_API_ROUTES.remoteScreen.session(sessionId)}/stream`,
} as const;
