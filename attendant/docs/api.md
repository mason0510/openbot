# Attendant 对外接口

桌面和手机用这套接口登录、登记电脑、出手机连接票、出 Signal 票。聊天正文、文件和命令不经过本服务。模型回复在电脑本地。

可导入的请求集合在同目录 `attendant-api.postman_collection.json`。

## 入口

| 项 | 值 |
| --- | --- |
| 基址 | `https://api-attendant.mwcpool.uk` |
| Signal | `wss://api-attendant.mwcpool.uk/signal` |

不要把请求打到 `api.openbot.run`、`signal.openbot.run` 或 `https://api.attendant.mwcpool.uk`。后一个主机名不是当前入口。

## 约定

- HTTP/1.1。一个连接一个请求，响应后关闭。不支持 chunked。正文最多 64 KiB，必须带 `Content-Length`。
- 服务不检查 `Content-Type`。请仍发送 `application/json`。
- 需要登录的接口使用 `Authorization: Bearer <sessionToken>`。邮箱会话 30 天。
- `expiresAt`、`resendAt`、`createdAt` 是 Unix 毫秒。JWT 和 `peer-ready` 里的 `sessionExpiresAt` 是 Unix 秒。
- 邮箱接口失败是字符串：`{"error":"invalid_email"}`。
- 主机、手机票、远程会话失败是对象：`{"error":{"code":"...","message":"..."}}`。
- 未知路径是 `{"error":"not_found"}`。
- 成功的 204 没有正文。

## 接入顺序

```
桌面登录
  POST /v1/auth/email/start
  POST /v1/auth/email/verify          → sessionToken
  POST /v2/remote/hosts/register      → machineToken
  POST /v2/remote/hosts/{hostId}/ticket
  WebSocket hello peer=host

手机登录（同一邮箱）
  POST /v1/auth/email/start
  POST /v1/mobile-auth/email/verify   → sessionToken
  GET  /v2/remote/hosts/
  POST /v2/remote/sessions/           → sessionId
  POST /v2/remote/sessions/{sessionId}/ticket
  WebSocket hello peer=client

服务
  先向电脑发 peer-ready
  再向手机发 ready
两端自己建立 WebRTC。本服务不转发媒体。
```

远程会话和 Signal 票只在进程内存里。服务重启后要重新开会话、重新出票。邮箱会话还在。

## 健康与公钥

### `GET /health/live`

不需要登录。

输出 `200`：

```json
{"status":"ok"}
```

### `GET /.well-known/jwks.json`

不需要登录。桌面用它验证手机 Signal 票。响应里没有私钥。

输出 `200`：一个 P-256 密钥。`kid` 为 `attendant-1`，`alg` 为 `ES256`，`use` 为 `sig`。

## 邮箱登录

### `POST /v1/auth/email/start`

不需要登录。向邮箱发送 8 位码。

输入：

| 字段 | 要求 |
| --- | --- |
| `email` | 必填。去空白后转小写。须含 `@` 和域名中的 `.`。总长不超过 254。拒绝空白、控制字符、引号、反斜杠。中文可留在本地部分。 |

输出 `200`：

| 字段 | 含义 |
| --- | --- |
| `challengeId` | 下一步校验用 |
| `expiresAt` | Unix 毫秒，发出后 600 秒 |
| `resendAt` | Unix 毫秒，发出后 60 秒 |

60 秒内对同一邮箱再调，返回同一个 `challengeId`，不发第二封信。

失败：`400 invalid_email`，`500 mail_failed`，`503 rate_limit`。后一个表示这封码还活着且已错满 5 次。

码字符集不含 `0`、`1`、`I`、`O`。可写成 `ABCD-EFGH`。明文不入库。

### `POST /v1/auth/email/verify`

### `POST /v1/mobile-auth/email/verify`

两条是同一个处理。不需要登录。手机多送的设备字段会被忽略。不要送邮箱。

输入：

| 字段 | 要求 |
| --- | --- |
| `challengeId` | 必填 |
| `code` | 8 位。允许连字符、空白和小写。服务规范化后再比对。 |

输出 `200`：

| 字段 | 含义 |
| --- | --- |
| `sessionToken` | 放进 `Authorization: Bearer` |
| `user.id` | 邮箱的 SHA-256 十六进制，稳定不变 |
| `user.email` | 小写邮箱 |
| `user.name` | `null` |
| `user.avatarUrl` | `null` |
| `user.analyticsEnabled` | `null` |

失败：`400 invalid_challenge`、`400 invalid_code`、`400 code_expired`、`404 no_code`、`429 too_many_attempts`、`500 mail_failed`。成功后这封码作废。

## 当前用户

### `GET /v1/me`

需要 Bearer。输出 `200` 是用户对象本身，没有外层包装。字段与校验成功时的 `user` 相同。

失败：`401 missing_token`，`401 invalid_token`。未知和超过 30 天都是 `invalid_token`。

### `GET /v1/mobile-auth/session`

需要 Bearer。输出与 `GET /v1/me` 相同。失败是嵌套 `401 unauthorized`。

### `DELETE /v1/mobile-auth/session`

需要 Bearer。丢掉这张会话。输出 `204`。不关闭已经打开的 Signal。失败：`401 unauthorized`。

## 手机连接票

这组票是桌面展示、手机兑换的短码。它不是 Signal JWT。

### `POST /v1/mobile-auth/ticket`

需要 Bearer。同一邮箱和同一 `hostId` 只留一张活码。新码替换旧码。明文不入库。

输入：

| 字段 | 要求 |
| --- | --- |
| `hostId` | 1 到 128 字符，仅字母、数字、连字符 |
| `fingerprint` | 正好 43 个 base64url 字符 |

输出 `200`：`ticket` 为 43 字符 base64url，`expiresAt` 为 Unix 毫秒，120 秒后到期。

失败：`401 unauthorized`，`400 invalid_mobile_host`，`409 mobile_host_mismatch`，`500 store_failed`。

### `DELETE /v1/mobile-auth/ticket?host={hostId}`

需要 Bearer。没有活码也返回 `204`。不取消别人的码。

失败：`401 unauthorized`，`400 invalid_mobile_host`，`500 store_failed`。

### `POST /v1/mobile-auth/redeem`

不需要旧 Bearer。码只用一次。设备字段不合法时，码不消耗。

输入：

| 字段 | 要求 |
| --- | --- |
| `ticket` | 必填。32 到 128 字符，仅字母、数字、`_`、`-`。发出的码是 43 字符 |
| `deviceId` | UUID 第 4 版 |
| `deviceName` | 去空白后 1 到 80 个字符，不能有控制字符。中文可以 |
| `platform` | 只能是 `ios`、`android`、`unknown` |

输出 `200`：`sessionToken`、`user`、`host.hostId`、`host.fingerprint`。

失败：`400 invalid_mobile_ticket`，`400 invalid_mobile_device`，`401 invalid_mobile_ticket`，`500 store_failed`。未知、过期或第二次使用是 `401`，不建会话。

### `GET /v1/mobile-auth/devices`

需要 Bearer。当前没有设备行。

| 查询 | 输出 `200` |
| --- | --- |
| `host={hostId}` | `{"devices":[]}` |
| `includeDesktop=true` 且没有 `host` | `{"sessions":[]}` |

未登记的合法 `host` 也是空列表。失败：`401 unauthorized`，`400 invalid_mobile_host`，`409 remote_host_taken`。

### `POST /v1/mobile-auth/link`

需要 Bearer。返回一条持久连接链。服务不保存这条链。

输入：`hostId`、`fingerprint`，规则同上。

输出 `200`：`link` 为 43 字符，字符集 `[A-Za-z0-9_-]`。`createdAt` 为 Unix 毫秒。

失败：`401 unauthorized`，`400 invalid_mobile_host`，`409 remote_host_taken`，`500 store_failed`。

### `DELETE /v1/mobile-auth/link`

当前直接返回 `204`。不读登录头，不删任何数据。桌面若在本地保存了链，须自己丢掉。

## 电脑主机

### `POST /v2/remote/hosts/register`

需要 Bearer。一台 `hostId` 只属于一个邮箱。同一邮箱再次登记是原地更新。

输入：

| 字段 | 要求 |
| --- | --- |
| `hostId` | 1 到 128，仅字母、数字、连字符 |
| `name` | 1 到 80。不能含制表符、换行、引号、反斜杠 |
| `ownerMembershipId` | 可空。非空时不超过 128，仅字母、数字、`_`、`-`。不合法时服务派生 32 位十六进制 |
| `devicePublicKey` | 不能含真换行、回车、引号、反斜杠、制表符，且不超过 4096 字符。带真换行的 PEM 会 `400`，旧行不会被这次请求改掉 |
| `machineToken` | 更新时送上一次返回的值。首次可空 |
| `rotateCredential` | `true` 时换新的 `machineToken` |

公钥变化时 `authEpoch` 加 1，最小为 1。

输出 `200`：`hostId`、`name`、`membershipId`、`authEpoch`、`machineToken`。`machineToken` 是 64 位十六进制。不要把它写进日志或转发的文档。

失败：`401 unauthorized`，`401 stale_machine_token`，`400 invalid_remote_host`，`409 remote_host_taken`，`500 store_failed`。

不要用示例公钥对已经在用的电脑调用本接口。那会改掉库里的公钥。

### `GET /v2/remote/hosts/`

`/v2/remote/hosts` 相同。需要 Bearer。只返回当前邮箱的电脑。

输出 `200` 的每一项：

| 字段 | 值 |
| --- | --- |
| `hostId`、`name` | 登记时的值 |
| `logoKey` | `null` |
| `devicePublicKey` | 公钥字符串，或 `null` |
| `keyHandovers` | 空数组 |
| `authEpoch` | 整数，至少为 1 |
| `membershipId` | 成员 id |
| `role` | `owner` |

失败：`401 unauthorized`。

旧数据可能把 JSON 换行存成字母 `n`。调用方若要校验 Ed25519，须能把这种 113 字符的扁平 PEM 还原成两行 PEM。只认真换行的安装包会报无效公钥。

### `GET /v2/remote/hosts/{hostId}/members/`

`/members` 相同。需要 Bearer。当前只返回所有者一行。

输出 `200`：`members` 含 `membershipId`、`email`、`name`、`avatarUrl`（`null`）、`role`（`owner`）、`status`（`active`）、`createdAt`（Unix 毫秒）。

失败：`401 unauthorized`，`400 invalid_mobile_host`，`404 remote_host_missing`，`409 remote_host_taken`。

### `POST /v2/remote/hosts/{hostId}/ticket`

桌面用机器凭证换 Signal 主机票。不读 Bearer。票在内存里，重启即失效。

输入：`machineToken`。

输出 `200`：`ticket`、`expiresAt`（Unix 毫秒，86400 秒后）、`signalUrl`。

失败：`400 invalid_remote_request`，`401 stale_machine_token`，`404 remote_host_missing`，`500 store_failed`。

下一步 `hello` 的 `peer` 必须是 `host`。主机不会收到 `peer-ready`。

## 手机远程会话

### `POST /v2/remote/sessions/`

`/v2/remote/sessions` 相同。需要 Bearer。行只在内存里。同一邮箱和同一 `hostId` 在 24 小时内复用同一 `sessionId`。

输入：`hostId`。

输出 `201`：`sessionId`、`hostId`、`expiresAt`（Unix 毫秒，创建后 24 小时）。

失败：`401 unauthorized`，`400 invalid_remote_request`，`404 remote_host_missing`，`403 forbidden`，`500 store_failed`。

### `POST /v2/remote/sessions/{sessionId}/ticket`

需要 Bearer。签发 ES256 JWT，并在内存里登记为客户端票。

输入：`clientPublicKey`。Ed25519 SPKI PEM。允许换行和回车。其他控制字符拒绝。空或超过 8192 字符拒绝。JSON 里的换行写成 `\n`。

输出 `200`：`ticket`、`expiresAt`（Unix 毫秒）、`signalUrl`、`hostId`。

JWT 载荷：

| 声明 | 值 |
| --- | --- |
| `aud` | `openbot-remote` |
| `jti` | 随机 |
| `sessionId`、`hostId`、`userId`、`membershipId` | 与这次会话和主机一致 |
| `role` | `owner` |
| `authEpoch` | 主机当前值 |
| `protocolMinimum`、`protocolMaximum` | 都是 `2` |
| `sessionExpiresAt` | Unix 秒。必须与随后 `peer-ready` 里的秒数相同 |
| `clientPublicKey` | 与输入逐字相同，含 PEM 换行 |
| `iat`、`exp` | Unix 秒。`exp` 不超过会话到期 |

头为 `alg=ES256`，`kid=attendant-1`。不要加 `signal` 声明。

失败：`401 unauthorized`，`400 invalid_remote_request`，`403 session_inactive`，`404 remote_host_missing`，`500 store_failed`。

进程重启后旧 `sessionId` 出票会 `403 session_inactive`。

下一步 `hello` 的 `peer` 必须是 `client`，`token` 是这张 JWT。

### `POST /v2/remote/sessions/{sessionId}/end`

需要 Bearer。未知会话也返回 `204`。另一个邮箱的会话返回 `403 forbidden`。不关闭已经打开的 Signal。

## 功能与托管

### `GET /v1/mobile/features`

不需要登录。固定输出：

```json
{"features":{"cloudServers":false,"browser":false},"latestVersion":null}
```

### `GET /v2/hosting/servers/`

不需要登录。这台部署没有云主机。

```json
{"available":false,"lifecycleAvailable":false,"servers":[],"maxServers":0}
```

### `/v2/hosting/servers/{hostId}/wake`

该前缀下除列表以外的路径，任何方法都返回 `404`：

```json
{"error":{"code":"hosted_server_not_found","message":"This host is not a cloud server."}}
```

这是预期结果。这台电脑不是云主机。

## Signal

`GET /signal` 在带 `Upgrade: websocket` 和非空 `Sec-WebSocket-Key` 时返回 `101`。普通 POST 不是这条协议。

第一条文本帧必须是 hello：

```json
{"type":"hello","version":1,"token":"<ticket>","peer":"host"}
```

手机把 `peer` 写成 `client`。`peer` 必须和出票时登记的角色相同。

手机先到、电脑尚未 hello 时，手机收到：

```json
{"type":"host-waiting","version":1,"resumeToken":"<base64url>"}
```

两端都在房间后，服务先给电脑：

```json
{
  "type": "peer-ready",
  "version": 1,
  "connectionId": "<id>",
  "sessionId": "<id>",
  "userId": "<sha256>",
  "membershipId": "<id>",
  "role": "owner",
  "sessionExpiresAt": 1793786400,
  "resumed": false
}
```

`sessionExpiresAt` 是秒，并且必须和 JWT 里的秒数相同。然后给手机：

```json
{
  "type": "ready",
  "version": 1,
  "connectionId": "<id>",
  "resumeToken": "<base64url>",
  "iceServers": [
    {"urls": "stun:<TURN_HOST>:3478"},
    {
      "urls": [
        "turn:<TURN_HOST>:3478?transport=udp",
        "turn:<TURN_HOST>:3478?transport=tcp",
        "turns:<TURN_HOST>:5349?transport=tcp"
      ],
      "username": "<expirationSeconds>:<sessionId>",
      "credential": "<hmac-sha1-base64>"
    },
    {"urls": "stun:<lan-ipv4>:3478"}
  ]
}
```

当未设置 `TURN_HOST` + `TURN_SHARED_SECRET` 时，`iceServers` 只含主机 `address-update` 发布的局域网 STUN（每族最多一条）；两者都未发布时列表可为空。设置后与官方 Signal 同形：先 STUN/TURN/TURNS 中继票据，再附加局域网 STUN。`username` 为 `expiration:sessionId`（主机票无 sessionId 时用 `hostId`），`credential` 为 `HMAC-SHA1(TURN_SHARED_SECRET, username)` 的标准 Base64。默认凭证 TTL 3600 秒（可用 `TURN_CREDENTIAL_TTL_SECONDS` 覆盖）；客户端可发 `turn-refresh` 再取一帧带新票据的 `ready`。

之后的帧原样转发给房间里的另一端。本服务不创建 WebRTC。`resumeToken` 可代替原票再次 hello。

失败帧：

```json
{"type":"error","version":1,"code":"authentication_required","message":"The Signal ticket is not valid."}
```

`invalid_message` 表示 hello 不是第一条，或 `peer` 与票不一致。

## 注意事项

- 重启后，内存里的远程会话和 Signal 票作废。账号和邮箱会话还在。重新开会话并出票。
- 登记主机时不要发送含真换行的 PEM。那次请求会 `400`，库里的旧公钥保持不变。
- 读取主机列表时，调用方要能接受字母 `n` 代替换行的旧公钥。否则 Ed25519 校验失败，界面仍显示连接失败。
- `ready.iceServers`：配置了 `TURN_HOST` + `TURN_SHARED_SECRET` 时含官方同款 TURN 中继；另附主机 `address-update` 的局域网 STUN。未配置 TURN 时仅局域网 STUN。空的 `iceServers` 会被客户端拒绝。凭证勿写入日志。`GET /health/turn` 只报告是否已配置（不含密钥）。
- 客户端 JWT 的 `aud` 必须是 `openbot-remote`。`clientPublicKey` 必须和后续认证帧里的公钥逐字相同。
- `machineToken`、`sessionToken`、Signal 票和私钥不要写入日志、截图或对外转发的副本。
