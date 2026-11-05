import { defineMessages } from "../../../message";

export const messages = defineMessages("mobile.computer", {
  "mobile.computer.tab": "Computer",
  "mobile.computer.open": "Open",
  "mobile.computer.screenOf": "{name}'s screen",
  "mobile.computer.previewUnavailable": "Screen preview unavailable",
  "mobile.computer.openLive": "Open remote desktop",
  "mobile.computer.close": "Close remote desktop",
  "mobile.computer.refresh": "Refresh",
  "mobile.computer.keyboard": "Keyboard",
  "mobile.computer.cursor": "Cursor control",
  "mobile.computer.magnify": "Zoom",
  "mobile.computer.settings": "Remote desktop settings",
  "mobile.computer.status.connecting": "Connecting to the computer…",
  "mobile.computer.status.connected": "Connected",
  "mobile.computer.status.offline": "The server is not connected.",
  "mobile.computer.status.unsupported":
    "This server cannot share its screen on a phone. Update OpenBot on this server.",
  "mobile.computer.status.notSharing": "{name} is not sharing its screen",
  "mobile.computer.status.notSharingBody":
    "On that computer, open System Settings → Privacy & Security → Screen & System Audio Recording, turn on OpenBot (or Slavebot), then try again here.",
  "mobile.computer.status.failed": "Could not open the desktop.",
  "mobile.computer.status.streamNotReady":
    "The server did not start the stream. Sunshine may have stopped on the server. Try again.",
  "mobile.computer.retry": "Try again",
  "mobile.computer.disconnect": "Disconnect",
  "mobile.computer.header": "Computer",
  "mobile.computer.detailsTab": "Details",
  "mobile.computer.libraryTab": "Library",
  "mobile.computer.computerTab": "Computer",
});
