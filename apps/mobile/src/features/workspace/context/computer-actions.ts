import type { RemoteDesktopCapabilities, RemoteDesktopSession } from "@openbot/contracts/ipc";
import {
  decodeRemoteDesktopCapabilities,
  decodeRemoteDesktopSession,
  ignoreRemoteDesktopResponse,
  REMOTE_DESKTOP_CAPABILITY,
  remoteDesktopRoutes,
  supportsRemoteDesktop,
} from "@/features/computer/model/remote-desktop-session";
import type { RemoteDesktopViewerEvent } from "@/features/computer/model/remote-desktop-viewer-bridge";
import type { RemoteTeamTransportRef } from "@/features/workspace/components/remote-team-transport";
import type { WorkspaceRequest } from "@/features/workspace/context/host-request-actions";
import { hasCapability, type ServerCapabilities } from "@/features/workspace/model/server-capabilities";

export type MobileRemoteDesktopSupport = {
  /** Host advertises `remote-desktop`. */
  available: boolean;
};

export type MobileComputerActions = {
  remoteDesktopSupport: (serverId: string) => MobileRemoteDesktopSupport;
  loadRemoteDesktopCapabilities: (serverId: string) => Promise<RemoteDesktopCapabilities>;
  startRemoteDesktopSession: (serverId: string) => Promise<RemoteDesktopSession>;
  stopRemoteDesktopSession: (serverId: string, sessionId: string) => Promise<void>;
  openRemoteDesktopViewer: (
    serverId: string,
    session: RemoteDesktopSession,
    listener: (event: RemoteDesktopViewerEvent) => void,
  ) => { close: () => void; refresh: () => void } | null;
};

/** Remote desktop session and Moonlight viewer for a host that advertises `remote-desktop`. */
export function createComputerActions({
  request,
  client,
  capabilities,
}: {
  request: WorkspaceRequest;
  client: (serverId: string) => RemoteTeamTransportRef | undefined;
  capabilities: ServerCapabilities;
}): MobileComputerActions {
  return {
    remoteDesktopSupport: (serverId) => ({
      available: hasCapability(capabilities, serverId, REMOTE_DESKTOP_CAPABILITY),
    }),
    loadRemoteDesktopCapabilities: async (serverId) => {
      if (!supportsRemoteDesktop(capabilities.get(serverId))) {
        throw new Error("unsupported");
      }
      return request("GET", remoteDesktopRoutes.capabilities, decodeRemoteDesktopCapabilities, undefined, serverId);
    },
    startRemoteDesktopSession: async (serverId) => {
      if (!supportsRemoteDesktop(capabilities.get(serverId))) {
        throw new Error("unsupported");
      }
      return request("POST", remoteDesktopRoutes.sessions, decodeRemoteDesktopSession, {}, serverId);
    },
    stopRemoteDesktopSession: async (serverId, sessionId) => {
      await request("DELETE", remoteDesktopRoutes.session(sessionId), ignoreRemoteDesktopResponse, undefined, serverId);
    },
    openRemoteDesktopViewer: (serverId, session, listener) => {
      const peer = client(serverId);
      if (!peer || !supportsRemoteDesktop(capabilities.get(serverId))) return null;
      return peer.openRemoteDesktopViewer(session, listener);
    },
  };
}
