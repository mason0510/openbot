import { isDynamicRecord, isString } from "@openbot/contracts/runtime-values";
import { TEAM_API_ROUTES } from "@openbot/contracts/team-api-routes";
import {
  decodeRemoteDesktopSignalBinary,
  decodeRemoteDesktopSignalControl,
  encodeRemoteDesktopSignalControl,
} from "@openbot/contracts/team-protocol/remote-stream-v1";
import type { TeamProtocolV2Json } from "@openbot/contracts/team-protocol/v2";
import { createTeamRequestId } from "@openbot/team-client/request-id";
import { remoteDesktopRoutes } from "./remote-desktop-session";
import type { RemoteDesktopViewerCommand, RemoteDesktopViewerEvent } from "./remote-desktop-viewer-bridge";

type PeerRequest = (method: string, path: string, body?: TeamProtocolV2Json) => Promise<unknown>;
type PeerSend = (data: string | ArrayBuffer) => Promise<void>;

interface OpenViewer {
  viewId: string;
  sessionId: string;
  streamId: string;
  streamPath: string;
  grant: string;
  ready: boolean;
  iframe: HTMLIFrameElement | null;
  overlay: HTMLDivElement | null;
}

function readCommand(value: unknown): RemoteDesktopViewerCommand | null {
  if (!isDynamicRecord(value) || !isString(value.type) || !isString(value.viewId)) return null;
  if (value.type === "close" || value.type === "refresh") return { type: value.type, viewId: value.viewId };
  if (
    value.type === "open" &&
    isDynamicRecord(value.session) &&
    isString(value.session.id) &&
    isString(value.session.viewerGrant) &&
    isString(value.session.viewerUrl) &&
    isString(value.streamPath)
  ) {
    return {
      type: "open",
      viewId: value.viewId,
      session: {
        id: value.session.id,
        viewerGrant: value.session.viewerGrant,
        viewerUrl: value.session.viewerUrl,
      },
      streamPath: value.streamPath,
    };
  }
  return null;
}

/**
 * Moonlight remote desktop inside the peer page. HTTP assets go through Team API; the stream
 * WebSocket is tunneled on the desktop data channel — the same path the desktop viewer proxy uses.
 */
export function createRemoteDesktopViewer(
  send: PeerSend,
  request: PeerRequest,
  emit: (event: RemoteDesktopViewerEvent) => void,
) {
  let viewer: OpenViewer | null = null;

  function tearDown(reason: string | null) {
    const current = viewer;
    viewer = null;
    if (!current) return;
    current.iframe?.remove();
    current.overlay?.remove();
    void send(encodeRemoteDesktopSignalControl({ type: "close", streamId: current.streamId })).catch(() => undefined);
    emit({ type: "ended", viewId: current.viewId, reason });
  }

  async function authorize(sessionId: string, grant: string) {
    await request("POST", remoteDesktopRoutes.authorize(sessionId), { grant });
  }

  async function openStream(current: OpenViewer) {
    emit({ type: "state", viewId: current.viewId, state: "connecting" });
    await send(
      encodeRemoteDesktopSignalControl({
        type: "open",
        streamId: current.streamId,
        path: current.streamPath,
      }),
    );
  }

  function mountShell(current: OpenViewer) {
    const overlay = document.createElement("div");
    overlay.setAttribute("data-openbot-remote-desktop", current.viewId);
    overlay.style.cssText =
      "position:fixed;inset:0;z-index:2147483646;background:#090b0c;display:flex;flex-direction:column;";
    const iframe = document.createElement("iframe");
    iframe.title = "Remote desktop";
    iframe.allow = "autoplay; clipboard-read; clipboard-write; display-capture";
    iframe.style.cssText = "flex:1;width:100%;height:100%;border:0;background:#090b0c;";
    const bootstrap = `<!doctype html><meta charset="utf-8"><title>OpenBot Remote</title>
<meta name="color-scheme" content="dark">
<style>html,body{margin:0;width:100%;height:100%;background:#090b0c;color:#fff;font:14px system-ui}main{display:grid;place-items:center;height:100%;padding:24px;text-align:center}</style>
<main id="status">Connecting…</main>
<script>
const viewId = ${JSON.stringify(current.viewId)};
const status = document.getElementById("status");
window.addEventListener("message", (event) => {
  const data = event.data;
  if (!data || data.source !== "openbot-remote-desktop-host" || data.viewId !== viewId) return;
  if (data.type === "status" || data.type === "error") status.textContent = data.message || "";
});
parent.postMessage({ source: "openbot-remote-desktop-frame", type: "ready", viewId }, "*");
</script>`;
    iframe.srcdoc = bootstrap;
    overlay.appendChild(iframe);
    document.body.appendChild(overlay);
    current.overlay = overlay;
    current.iframe = iframe;
  }

  async function handle(command: RemoteDesktopViewerCommand) {
    if (command.type === "close") {
      if (viewer?.viewId === command.viewId) tearDown(null);
      return;
    }
    if (command.type === "refresh") {
      if (!viewer || viewer.viewId !== command.viewId) return;
      const current = viewer;
      current.ready = false;
      await send(encodeRemoteDesktopSignalControl({ type: "close", streamId: current.streamId })).catch(
        () => undefined,
      );
      current.streamId = createTeamRequestId((size) => crypto.getRandomValues(new Uint8Array(size)));
      try {
        await openStream(current);
      } catch (error) {
        emit({
          type: "state",
          viewId: current.viewId,
          state: "error",
          message: error instanceof Error ? error.message : String(error),
        });
      }
      return;
    }
    tearDown(null);
    const next: OpenViewer = {
      viewId: command.viewId,
      sessionId: command.session.id,
      streamId: createTeamRequestId((size) => crypto.getRandomValues(new Uint8Array(size))),
      streamPath: command.streamPath,
      grant: command.session.viewerGrant,
      ready: false,
      iframe: null,
      overlay: null,
    };
    viewer = next;
    try {
      await authorize(next.sessionId, next.grant);
      mountShell(next);
      await openStream(next);
    } catch (error) {
      emit({
        type: "state",
        viewId: next.viewId,
        state: "error",
        message: error instanceof Error ? error.message : String(error),
      });
      tearDown(error instanceof Error ? error.message : String(error));
    }
  }

  return {
    async handleCommand(value: unknown) {
      const command = readCommand(value);
      if (command) await handle(command);
    },
    handle,
    receive(data: string | ArrayBuffer) {
      const current = viewer;
      if (!current) return;
      try {
        if (typeof data === "string") {
          const control = decodeRemoteDesktopSignalControl(data);
          if (control.streamId !== current.streamId) return;
          if (control.type === "opened") {
            current.ready = true;
            emit({ type: "state", viewId: current.viewId, state: "connected" });
            current.iframe?.contentWindow?.postMessage(
              {
                source: "openbot-remote-desktop-host",
                type: "status",
                viewId: current.viewId,
                message: "Connected. Starting screen stream…",
              },
              "*",
            );
            void request("GET", TEAM_API_ROUTES.remoteScreen.viewer(current.sessionId))
              .then((body) => {
                if (!viewer || viewer.viewId !== current.viewId) return;
                if (isDynamicRecord(body) && isString(body.base64)) {
                  const html = atob(body.base64);
                  const doc = current.iframe?.contentDocument;
                  if (doc) {
                    doc.open();
                    doc.write(html);
                    doc.close();
                  }
                } else {
                  current.iframe?.contentWindow?.postMessage(
                    {
                      source: "openbot-remote-desktop-host",
                      type: "status",
                      viewId: current.viewId,
                      message: "Stream connected. Waiting for Moonlight…",
                    },
                    "*",
                  );
                }
              })
              .catch(() => undefined);
          }
          if (control.type === "close" || control.type === "error") {
            emit({
              type: "state",
              viewId: current.viewId,
              state: "error",
              message: control.type === "error" ? control.message : control.reason,
            });
            tearDown(control.type === "error" ? control.message : (control.reason ?? null));
          }
        } else {
          decodeRemoteDesktopSignalBinary(data);
        }
      } catch {
        tearDown("Remote desktop signal failed.");
      }
    },
    disconnect() {
      tearDown(null);
    },
  };
}
