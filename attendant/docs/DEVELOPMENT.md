# 开发经验与下一步

这次把桌面邮箱登录、手机扫码票据、主机注册三条路接到同一套 std-only 服务上。手机已经能聊天，是因为电脑正式端的主机在线。要把同一条路完全走自己的 API，还缺 Signal 票据。

## 这次怎么排障

固定顺序：界面原话 → 桌面 IPC / 手机截图 → 对照桌面源码的 URL 和解码器 → 本服务缺哪条路由或哪段形状 → 写成 Rust → 在 Angel 本机 curl 回读 → 再点一次桌面或手机。不要先猜，不要先重编客户端。

证据落点：

- 桌面测试窗：`/tmp/openbot-attendant-client/logs/trace.ndjson`（IPC 名和 outcome）。`main.log` 几乎没有这次点击。
- 手机：`adb exec-out screencap`，不要用系统相机当验收。
- 服务：Angel 上 `curl` 到 `127.0.0.1:8791`。进程默认不打每条 HTTP。

### 1. 登录服务缺失

现象：桌面「账号服务返回了错误。」邮箱码位数不对、或点连接手机立刻失败。

原理：桌面只认两种 body。`{"error":"字符串"}` 一律变成那句固定文案。嵌套 `{"error":{"code","message"}}` 才显示服务端句子。未知路径是字符串 `not_found`，所以缺路由看起来像账号服务坏了。

还原：

- 邮箱：读桌面 `createEmailChallenge` / `verifyEmailChallenge`。必须返回 `challengeId`、`expiresAt`（unix 毫秒）、校验后 `sessionToken` + `user.id`。OTP 是 8 格，6 位码填不满。
- 读请求必须读到 `Content-Length`。只读一次 socket 会丢掉 body，verify 变成空。
- 连接手机前还有三条：`POST /v2/remote/hosts/register`、`GET /v2/remote/hosts/`、`GET /v1/mobile-auth/devices`。缺任何一条就是字符串 404。

测试：登录成功后查 TSV 有 `SESS`。部署三条主机路由后，`auth:list-mobile-connected-devices` 从 error 变成 ok，客户端不用重编。

### 2. 二维码不创建 / 服务器连不上

现象：点「在手机上登录」没有码，或码出了但扫了没反应。

原理：码不是独立接口。顺序是 list devices → `host.start`（register → members → Signal 票）→ 主机 `online` 才 `POST /v1/mobile-auth/ticket`。票是 43 位无填充 base64url，`expiresAt` 必须是毫秒。系统相机不打开 `openbot://`。开发页登录是 `expo.dev`，不是本服务。

还原：先补 devices / register / hosts，让 list 不再挡在 `host.start` 前面。`POST /v1/mobile-auth/ticket` 和 redeem 已经按桌面解码器写好。扫码入口是应用内扫描，不是系统相机。

测试：桌面 `auth:create-mobile-connect` 从持续 error 变成能出码。手机扫码后电脑设备列表出现「MI 9 Transparent Edition · 上次活跃：刚刚」。这只证明账号会话在。

未完成：`POST /v2/remote/hosts/{hostId}/ticket`（Signal）和 members。没有它们，自己的 API 上 `host.start` 仍会 404。这次能聊天，走的是电脑正式窗口的官方实时通道。

### 3. 手机连不上

现象：白屏、`fail to connect 127.0.0.1:8081`、搜索「无法加载全部结果」、服务器设置「主机离线」。

原理：调试 APK 不带 JS，必须 `adb reverse tcp:8081 tcp:8081` 加载 Metro。无线调试端口一变，转发就没了。安装包里若 `libappmodules.so` 等是 24 字节空文件，JS 起不来。账号列表和主机在线是两条路：列表走账号 API，搜索和聊天走电脑主机。测试窗口退出后，手机会一直显示离线。

还原：用已链接的 ELF 重打包 APK，不要用被 ccache / 并行 ninja 写零的 `.so`。Metro 要听 IPv4 `*:8081`，不要只听 `[::1]`。转发丢了就再 `adb reverse`，不要重编。

测试：截图看到 OpenBot 搜索页或服务器设置，而不是 Expo 启动页。日志有 `Running "main"`。转发恢复后 8081 报错消失。搜索失败且「主机离线」时，先看电脑 OpenBot 窗口还在不在，不要先重装手机。

### 4. 打前端包（Android 调试 APK）

这件事最耗时。根因几乎全在原生库和调试加载，不在业务代码。下次不要从 `clean` 开始，不要并行编 `.so`。

发现：

- `assembleDebug` 成功，装上却白屏或 JS 不起。APK 里 `libappmodules.so`、`libreanimated.so` 等 10 个库是 24 字节空文件，或同样大小但 magic 是 `00 00 00 00`。
- Homebrew `ccache` 缺 `libfmt.11.dylib` 时当启动器，会把产物写成全零。
- `ninja -j8` 会得到「体积对、不是 ELF」的 `.so`。`ninja -j1` 才链出真 ELF。
- Gradle `--build-cache` 会把空库再灌回来。
- JetBrains JBR 17 没有 `jlink`。daemon 用 JDK 21，toolchain 17 要用带 `jlink` 的 Temurin。
- 调试 APK 不带 JS。手机必须 `adb reverse tcp:8081 tcp:8081` 加载 Metro。Metro 若只听 `[::1]`，转发连的是 `127.0.0.1`，就是 `fail to connect 127.0.0.1:8081`。无线调试端口一变，转发清空。
- 系统相机扫 `openbot://` 没反应。开发启动页登录是 `expo.dev`。
- 小米「USB安装」关掉时 `INSTALL_FAILED_USER_RESTRICTED`。

原理：Expo dev client 把 JS 放 Metro，APK 只负责原生壳。壳里的 `.so` 是空的，React 起不来，后面所有登录、扫码都像「包坏了」。其实业务已经在 Metro 环境变量 `EXPO_PUBLIC_OPENBOT_API_URL` 里。

还原（已验证）：

1. 不要 `clean`，不要第二份 Gradle。
2. `ccache` 改成透传：`exec "$@"`。
3. 原生链接 `ninja -j1`。编完检查每个 `.so` 头四字节是 `7f 45 4c 46`。
4. 若 APK 已打好但库是空的：从 `cxx/.../obj/arm64-v8a/` 和各模块 `obj/arm64-v8a/` 取出真 ELF，写进 APK，`zipalign -p 4`，debug keystore 签名后再装。
5. Metro 听 `*:8081`（`--host lan`）。每次重连无线调试后重新 `adb reverse tcp:8081 tcp:8081`。
6. 打开应用用 `openbot://expo-development-client/?url=http://127.0.0.1:8081`，在应用里扫码。

不要为改账号 API 地址重编桌面。桌面用 `OPENBOT_AUTH_API_URL`。不要为 8081 失败重编 APK，先查转发。

可执行入口：`scripts/android-debug-pack.sh`。默认 `check` 只读。`pack` / `repack` / `connect` 才写。Workflow 名 `android-debug-pack`，参数 `mode` 默认 `check`。

## 算法

### OneTimeEmailCode

- 保证：每个小写邮箱同一时间一个码，10 分钟，单次使用。60 秒内重发返回同一个 `challengeId`，不发第二封信。
- 输入：邮箱。触发：`POST /v1/auth/email/start`。
- 停止：校验成功、过期、或 5 次失败。
- 不保证：邮箱可达。不保证桌面 OTP 槽以外的位数。
- 落点：`src/auth.rs` `start` / `verify`。明文不入库。盐 + SHA-256。

### MobileConnectTicket

- 保证：43 位无填充 base64url；`expiresAt` 是 unix 毫秒且大于客户端 `Date.now()`；同一 (email, hostId) 只有一张活票；坏设备字段在花票之前拒绝。
- 输入：桌面会话 + hostId + fingerprint。触发：`POST /v1/mobile-auth/ticket`。兑换：`POST /v1/mobile-auth/redeem`。
- 停止：120 秒、DELETE、或一次成功兑换。
- 不保证：Signal / WebRTC。不保证设备列表里出现这台手机。
- 落点：`src/auth.rs` `issue_mobile_ticket` / `redeem_mobile_ticket`。票明文不入库。

### HostRegister

- 保证：一个 hostId 只属于一个邮箱；`authEpoch` 从 1 起，公钥变化才加一；machine token 32 字节 hex。
- 输入：桌面会话 + hostId + name。触发：`POST /v2/remote/hosts/register`。
- 停止：写盘成功。
- 不保证：成员列表、Signal 票、主机在线。
- 落点：`src/auth.rs` `register_remote_host`，TSV `HOST` 九列。

## 桌面解码（不要改错形状）

桌面 `AuthApiError.fromResponseEffect`：

- `{"error":"字符串"}` 或非嵌套 body → 界面「账号服务返回了错误。」
- `{"error":{"code","message"}}` → 显示服务端 `message`

因此新路由必须嵌套错误。邮箱 start/verify/me 保持字符串错误，已经和桌面对齐。未知路径仍是字符串 `not_found`。缺路由时桌面看起来像账号服务坏了，其实是 404。

`json_string` 找正文里第一次出现的键。嵌套 `host.hostId` 能读到。不要改成只读顶层。

## 桌面点击顺序

`auth:create-mobile-connect`：

1. `GET /v1/mobile-auth/devices?host=` 未注册但合法的 hostId 必须 `{"devices":[]}`，不能 400。
2. `host.start`：`POST /v2/remote/hosts/register`，然后成员列表，然后 `POST /v2/remote/hosts/{hostId}/ticket`。
3. 主机 `online` 之后才 `POST /v1/mobile-auth/ticket`。

`GET /v1/mobile-auth/devices?includeDesktop=true` 且无 host 是另一个解码器：`{"sessions":[]}`。不要和 `{devices:[]}` 混。

`SESS` 保持四列。加列会让回滚的旧二进制丢掉会话。

## 部署

- 源码：本仓库。线上二进制：`attendant-api`。
- Angel 没有 rustc。在有 rustc 的 Linux 上 `bash -lc "cargo build --release"`（非 login shell 的 PATH 经常没有 cargo）。
- 安装前备份现二进制。新二进制就位后再 restart。
- 旧二进制会跳过 `HOST` 行，回滚不会丢 CODE / SESS / TICKET。
- 进程不打每条 HTTP。排障先看桌面 `trace.ndjson` 的 IPC 名和 outcome。

## 客户端踩坑

- 已安装的桌面不必为改 API 地址重编。用 `OPENBOT_AUTH_API_URL` 和独立 `--user-data-dir`。
- 调试 APK 不带 JS。手机必须 `adb reverse tcp:8081 tcp:8081` 加载 Metro。无线调试端口一变，转发就没了，界面变成 `fail to connect 127.0.0.1:8081`。
- 系统相机扫 `openbot://` 没反应。在应用里扫。
- 开发启动页右上角登录是 `expo.dev`，不是本服务。
- 账号列表里出现手机，只说明会话在。搜索和聊天要电脑主机在线。主机窗口退出后手机会显示离线。

## 下一步（按桌面调用顺序）

1. `POST /v2/remote/hosts/{hostId}/ticket` → `{ticket, expiresAt, signalUrl}`。`signalUrl` 必须是 `ws:` / `wss:`。没有这条，`host.start` 会 404 字符串错误。
2. `GET /v2/remote/hosts/{hostId}/members/`。桌面在发 Signal 票之前会拉成员。
3. `POST /v1/mobile-auth/link` → `{link, createdAt}`。桌面 `create-mobile-connect-link` 走这条。
4. 兑换成功后写入设备，让 `GET .../devices` 返回真实手机，而不是空数组。
5. 每条请求打一行日志（方法、路径、状态、耗时）。不要打 token、验证码、machine token。
6. 自建 Signal / TURN。没有它，聊天仍依赖电脑官方主机的实时通道。

新路由继续嵌套错误。不要改邮箱字符串错误。不要给 `SESS` 加列，除非明确放弃旧二进制回滚。


## TURN（跨网中继）

Attendant Signal 可选接入与官方 `remote/api` 同款的 coturn 长效票据：

- 必填成对：`TURN_HOST`、`TURN_SHARED_SECRET`
- 可选：`TURN_PORT`（默认 3478）、`TURN_TLS_PORT`（默认 5349）、`TURN_CREDENTIAL_TTL_SECONDS`（默认 3600）
- 未设置时：`ready` / `ice-servers` 只发主机 `address-update` 的局域网 STUN
- 已设置时：先发 `stun` + `turn`/`turns`（HMAC-SHA1），再附加局域网 STUN
- `GET /health/turn` 查看是否已配置（永不回传密钥）

跨网受限场景见上游 issue #2665 / #3898；凭证过期导致 CreatePermission 403 见 #4736（客户端应 `turn-refresh`）。
