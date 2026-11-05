# OpenBot / slavebot — feature/mobile-computer-tab 进度

更新时间：2026-11-05（America/Los_Angeles）

## 分支

- 本地：`feature/mobile-computer-tab`
- 远程跟踪：`fork/feature/mobile-computer-tab`
- 仓库路径：`/Volumes/codeMason/code/02-production/openbot`
- 同步树：`/Volumes/SlavebotBuild/openbot`（此前改动曾双向同步）

## 已完成

1. **电脑 tab（Grok 风格）**  
   预览 →「打开」全屏 WebRTC 远程桌面（刷新 / 键盘 / 光标等）。已合入本分支历史提交。

2. **品牌 / 包名 slavebot**  
   Android / iOS 包名改为 `app.slavebot.mobile`；通知模块包路径迁到 `app.slavebot.*`；文案与 changelog 同步。

3. **更新通知开关**  
   Settings → Updates 增加「更新通知」，默认 **关**（`notifyUpdates: false`）。关闭时不自动弹 provider CLI 更新提示。

4. **暂时隐藏扫码登录**  
   `apps/mobile/src/features/auth/model/feature-flags.ts`：`SHOW_QR_LOGIN = false`  
   登录页隐藏 Scan QR / Where is the QR code? / 邮箱页「用二维码」；邮箱登录升为主按钮。  
   恢复：把 `SHOW_QR_LOGIN` 改回 `true`。

5. **代理健壮性**  
   `system-http-proxy.ts`：TCP 连不上的 hop 会跳过并尝试下一个 host:port。

6. **云测**  
   BrowserStack 账号已用 `browserstack@mail.tap365.org` 激活；曾对 Pixel 8 做过启动冒烟（登录页）。电脑 tab 需登录后才能深测。

7. **APK**  
   曾在 APFS 卷 `SlavebotBuild` 打过 release APK（避开 exFAT 清零 `.so`）。路径参考：`/Users/houzi/tmp/slavebot-mobile-computer-tab.apk`；部分 APK 曾挪到 `/Volumes/SlavebotBuild/tmp-apks/`。  
   **隐藏扫码后尚未重编 APK**，真机要看到需再 `assembleRelease`。

## 未做 / 待办

- [ ] 隐藏扫码后的新版 APK 重编与安装
- [ ] 开机 splash：壁纸 + 约 6s 视频素材已备，**尚未接入** app
- [ ] 电脑 tab 登录后的真机 / 云测完整冒烟
- [ ] 提交后若需对外发布，再打正式包与商店流程

## 本次提交意图（相对 891c63d50）

- slavebot 包名与通知模块迁移
- 更新通知默认关
- 扫码登录入口 feature flag 隐藏
- 代理 hop 失败跳过
- changelog / i18n / settings 配套

## 恢复扫码登录

```ts
// apps/mobile/src/features/auth/model/feature-flags.ts
export const SHOW_QR_LOGIN = true;
```

## 备注

- 勿把签名密钥或 BrowserStack Access Key 写进本文件。
- EasyNote Play minSdk≥24 / 3.3.1 AAB 是另一条线，不在本仓库。
