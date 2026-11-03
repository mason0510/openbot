# Attendant

自己的账号 API。桌面和手机用它登录、出手机连接票、登记电脑主机。不依赖 OpenBot 官方账号服务。

实现是 Rust，只用标准库，没有 crates。进程绑 `127.0.0.1`，前面放反向代理。公网入口是 `https://api-attendant.mwcpool.uk`。对外接口见 [docs/api.md](docs/api.md)。

仓库没有许可证文件。不要再加回 `LICENSE`。

## 分支

| 分支 | 用途 |
| --- | --- |
| `master`、`main` | 同一条已上线的账号与主机路由，停在文档 `c1a59e2` |
| `dev` | 后续开发。打包脚本和这份说明在这里 |

新功能进 `dev`。不要直接改 `master`。

## 目录

| 路径 | 内容 |
| --- | --- |
| `src/main.rs` | 进程入口 |
| `src/http.rs` | HTTP/1.1、路由、读满 `Content-Length` |
| `src/auth.rs` | 邮箱码、会话、手机票、主机登记 |
| `src/store.rs` | `accounts.tsv` |
| `src/crypto.rs` | SHA-256、base64url、随机字节 |
| `src/mail.rs` | 发 8 位码 |
| `src/dns.rs` | MX / A 查询 |
| `scripts/android-debug-pack.sh` | 手机调试包：检查、链接、打包、重打包、转发 |
| `.grok/workflows/android-debug-pack.rhai` | 上面脚本的 workflow |
| `docs/api.md` | 对外接口说明 |
| `docs/attendant-api.postman_collection.json` | 可导入的 Postman 集合 |
| `docs/DEVELOPMENT.md` | 这次排障的经过和下一条缺口 |

## 三条算法

### OneTimeEmailCode

- 保证：一个小写邮箱同一时间一个码。10 分钟。用一次就作废。60 秒内重发返回同一个 `challengeId`，不发第二封信。5 次失败后拒绝。
- 输入：邮箱。触发：`POST /v1/auth/email/start`。停止：校验成功、过期、或失败次数用尽。
- 不保证：邮箱一定收得到。不保证 6 位码能填进桌面 8 格。
- 落点：`src/auth.rs` 的 `start` / `verify`。明文不入库。盐 + SHA-256。

### MobileConnectTicket

- 保证：票是 43 位无填充 base64url。`expiresAt` 是 unix 毫秒，必须大于客户端 `Date.now()`。同一邮箱和 hostId 只有一张活票。坏设备字段在花票之前拒绝。
- 输入：桌面会话、hostId、fingerprint。触发：`POST /v1/mobile-auth/ticket`。兑换：`POST /v1/mobile-auth/redeem`。
- 停止：120 秒、`DELETE`、或一次成功兑换。
- 不保证：Signal / WebRTC。不保证设备列表里出现这台手机。
- 落点：`issue_mobile_ticket` / `redeem_mobile_ticket`。票明文不入库。

### HostRegister

- 保证：一个 hostId 只属于一个邮箱。`authEpoch` 从 1 起，公钥变化才加一。machine token 是 32 字节 hex。
- 输入：桌面会话、hostId、name。触发：`POST /v2/remote/hosts/register`。
- 停止：写盘成功。
- 不保证：成员列表、Signal 票、主机在线。
- 落点：`register_remote_host`。TSV 的 `HOST` 是九列。

## 桌面怎么认错误

这是这次最容易踩的点。

- `{"error":"字符串"}` 或任何非嵌套 body：桌面显示「账号服务返回了错误。」
- `{"error":{"code","message"}}`：桌面显示服务端 `message`。

邮箱 start / verify / `/v1/me` 保持字符串错误，不要改成嵌套。新路由必须嵌套。未知路径仍是字符串 `{"error":"not_found"}`，所以缺路由看起来像账号服务坏了。

`json_string` 找正文里第一次出现的键。嵌套的 `host.hostId` 能读到。不要改成只读顶层。

## 已实现的路由

### 邮箱

`POST /v1/auth/email/start`，body `{"email"}`。

返回 `challengeId`、`expiresAt`、`resendAt`。时间是 unix 毫秒。码是 8 位。

`POST /v1/auth/email/verify` 和 `POST /v1/mobile-auth/email/verify`，body `{"challengeId","code"}`。

成功返回 `sessionToken` 和 `user`。`user.id` 是小写邮箱的 SHA-256 hex。`name` 和 `avatarUrl` 是 null。

`GET /v1/me`，头 `Authorization: Bearer <sessionToken>`。返回同一个 user。会话 30 天。

### 手机连接票

`POST /v1/mobile-auth/ticket`，桌面 Bearer。body 里带 `host.hostId` 和 `host.fingerprint`。

返回 `{"ticket","expiresAt"}`。

`DELETE /v1/mobile-auth/ticket?host=<hostId>`，桌面 Bearer。HTTP 204，空 body。

`POST /v1/mobile-auth/redeem`，无 Bearer。body：`ticket`、`deviceId`（UUID v4）、`deviceName`（1–80，去掉空白）、`platform`（`ios` | `android` | `unknown`）。

成功 `{"sessionToken","user","host"}`。第二次兑换 401。坏设备 400，并且不花票。

### 主机和设备列表

`POST /v2/remote/hosts/register`，桌面 Bearer。

字段：`hostId`、`name`、`ownerMembershipId`、`devicePublicKey`、`rotateCredential`、`machineToken`。

成功：`hostId`、`name`、`membershipId`、`authEpoch`（>= 1）、`machineToken`。

`GET /v2/remote/hosts/` 和 `GET /v2/remote/hosts`，桌面 Bearer。

`{"hosts":[...]}`。每项有 `logoKey: null`、`keyHandovers: []`、`role: "owner"`。

`GET /v1/mobile-auth/devices?host=<hostId>`，桌面 Bearer。`{"devices":[]}`。

hostId 合法但还没登记，也返回空列表，不能 400。另一个邮箱占着这个 hostId 才是 409。

`GET /v1/mobile-auth/devices?includeDesktop=true` 且没有 host，是另一个形状：`{"sessions":[]}`。不要和 `{devices:[]}` 混。

## 桌面点击顺序

点「在手机上登录」：

1. `GET /v1/mobile-auth/devices?host=`
2. `host.start`：先 `register`，再成员列表，再 `POST /v2/remote/hosts/{hostId}/ticket`
3. 主机状态是 online 之后，才 `POST /v1/mobile-auth/ticket`，这时才出二维码

所以没有码，不一定是出票接口坏了。前面任何一步 404 都会挡住。

## 还没有的路由

按桌面调用顺序补，都用嵌套错误：

1. `POST /v2/remote/hosts/{hostId}/ticket`，返回 `{ticket, expiresAt, signalUrl}`。`signalUrl` 必须是 `ws:` 或 `wss:`。没有它，自己的 API 上主机起不来。
2. `GET /v2/remote/hosts/{hostId}/members/`。发 Signal 票之前桌面会拉成员。
3. `POST /v1/mobile-auth/link`，返回 `{link, createdAt}`。
4. 兑换成功后把手机写进设备表，`devices` 不再永远是空数组。
5. 每条请求打一行：方法、路径、状态、耗时。不要打 token、验证码、machine token。

这次手机能聊天，是因为电脑正式窗口的主机在线。账号列表里出现手机，只说明会话在，不说明主机在线。

## 数据文件

`accounts.tsv`，进程用 flock 独占。

| 行 | 列数 | 内容 |
| --- | --- | --- |
| `CODE` | 8 | challenge id、邮箱、盐、哈希、创建秒、失败次数、上次发送秒 |
| `SESS` | 4 | `SESS`、邮箱、token、创建秒。不要加列，旧二进制会丢掉整行 |
| `TICKET` | 6 | 邮箱、host id、fingerprint、票的 SHA-256 hex、创建秒 |
| `HOST` | 9 | 邮箱、host id、name、membership id、auth epoch、machine token、公钥、创建秒 |

旧二进制会跳过 `HOST` 行。回滚二进制不会丢 CODE / SESS / TICKET。

hostId：1–128 位，`[A-Za-z0-9-]`。name 非空、不超过 80，且不能含 tab、换行、引号、反斜杠。

## 编译和部署

Angel 上没有 rustc。在有 cargo 的 Linux 登录 shell 里编：

```sh
bash -lc 'cd attendant-api && cargo build --release'
```

非登录 shell 的 PATH 经常找不到 cargo。

安装顺序：备份现二进制，放上新文件，再重启。不要先停服再等编译。

## 手机调试包

调试 APK 不带 JS。改账号地址不要重编桌面，桌面用 `OPENBOT_AUTH_API_URL`。手机用 Metro 的 `EXPO_PUBLIC_OPENBOT_API_URL`。

```sh
sh scripts/android-debug-pack.sh check
sh scripts/android-debug-pack.sh link
sh scripts/android-debug-pack.sh pack
sh scripts/android-debug-pack.sh repack
sh scripts/android-debug-pack.sh connect
```

`check` 只读。`link` 只用 `ninja -j1`。`pack` 发现已有 Gradle 就拒绝，也不 `clean`，也不开 build cache。`repack` 把 APK 里的空 `.so` 换成 obj 目录里的真 ELF，再对齐、签名。`connect` 只在 `127.0.0.1:8081` 已经是 Metro 时做 `adb reverse`。

库的前四字节必须是 `7f 45 4c 46`。24 字节或全零都是坏包。Homebrew ccache 缺库时会把产物写成零，脚本用透传 `ccache`。Metro 若只听 `[::1]`，手机上就是 `fail to connect 127.0.0.1:8081`，用 `--host lan` 重开，不要开第二个 Metro。

系统相机扫 `openbot://` 没反应。在应用里扫。开发启动页的登录是 expo.dev，不是本服务。

Workflow 名：`android-debug-pack`。参数 `mode` 默认 `check`，`repo` 指向本仓库检出目录。

## 这次三条故障

1. 登录服务缺失：缺路由返回字符串 `not_found`，桌面只显示「账号服务返回了错误。」补了邮箱合同、register、hosts、devices 之后，设备列表 IPC 从 error 变成 ok。桌面不用重编。
2. 二维码不创建：出码在 `host.start` 成功之后。list devices 对未登记主机必须是空数组。票的过期时间必须是毫秒。
3. 手机连不上：空 `.so`、8081 转发掉了、或电脑主机窗口已退出。先 `check` 和截图，不要先重装。

经过和验证记录在 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。
