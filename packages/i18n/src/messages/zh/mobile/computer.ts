import type { PartialTranslation } from "../../../message";
import type { messages as source } from "../../en/mobile/computer";

export const messages = {
  "mobile.computer.tab": "电脑",
  "mobile.computer.open": "打开",
  "mobile.computer.screenOf": "{name} 的屏幕",
  "mobile.computer.previewUnavailable": "无法预览屏幕",
  "mobile.computer.openLive": "打开远程桌面",
  "mobile.computer.close": "关闭远程桌面",
  "mobile.computer.refresh": "刷新",
  "mobile.computer.keyboard": "键盘",
  "mobile.computer.cursor": "光标控制",
  "mobile.computer.magnify": "放大",
  "mobile.computer.settings": "远程桌面设置",
  "mobile.computer.status.connecting": "正在连接电脑…",
  "mobile.computer.status.connected": "已连接",
  "mobile.computer.status.offline": "服务器未连接。",
  "mobile.computer.status.unsupported": "此服务器无法在手机上共享屏幕。请更新这台服务器上的 OpenBot。",
  "mobile.computer.status.notSharing": "{name} 没有共享屏幕",
  "mobile.computer.status.notSharingBody":
    "请在那台电脑上打开“系统设置 → 隐私与安全性 → 屏幕与系统音频录制”，开启 OpenBot（或 Slavebot），然后回到这里重试。",
  "mobile.computer.status.failed": "无法打开桌面。",
  "mobile.computer.status.streamNotReady": "服务器没有启动串流，Sunshine 可能已在服务器上停止，请重试。",
  "mobile.computer.retry": "重试",
  "mobile.computer.disconnect": "断开连接",
  "mobile.computer.header": "电脑",
  "mobile.computer.detailsTab": "详情",
  "mobile.computer.libraryTab": "资料库",
  "mobile.computer.computerTab": "电脑",
} as const satisfies PartialTranslation<typeof source>;
