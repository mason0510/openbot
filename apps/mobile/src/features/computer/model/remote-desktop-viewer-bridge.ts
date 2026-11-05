import type { RemoteDesktopSession } from "@openbot/contracts/ipc";

/** Commands the native Computer screen sends to the peer page that hosts Moonlight. */
export type RemoteDesktopViewerCommand =
  | {
      type: "open";
      viewId: string;
      session: Pick<RemoteDesktopSession, "id" | "viewerGrant" | "viewerUrl">;
      /** Path the host allowlists for the desktop signal tunnel. */
      streamPath: string;
    }
  | { type: "close"; viewId: string }
  | { type: "refresh"; viewId: string };

/** Events the peer page tells the Computer screen. */
export type RemoteDesktopViewerEvent =
  | { type: "state"; viewId: string; state: "connecting" | "connected" | "error"; message?: string }
  | { type: "ended"; viewId: string; reason: string | null };
