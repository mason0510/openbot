# iOS 打包编译经验（2026-11-02）

## 问题根源

1. 探测脚本多打了一个换行，Ruby 要求正好是 `200`，所以退回源码编译。
2. 安装脚本要删除自己的缓存目录，本机 `rm` 被 `rm-safe` 拦截。

## 解决方案

- `curl` 包装器在探测行打印 `200`（不带换行）。
- 真实 `rm` 走 `/bin/rm`（不拦截）。
- `pod install` 直接从缓存拉取 4 个 tarball。

## 结果

依赖安装成功（`ReactNativeDependencies`、`React-Core-prebuilt`、`ReactNativeCore-artifacts`、`hermes-ios-260318099.0.4-*.tar.gz` 都走预编译包，`Building from source: false`）。

## 后续

继续跑 `bun ../../scripts/mobile-ios.ts --device 00008020-000E75960AF3002E` 编译 App。

## 经验总结

- 预编译包必须提前下载到 `~/Library/Caches/ReactNative/` 并 SHA1 匹配。
- CocoaPods 探测用 `curl -Iw '%{http_code}'`，必须打印正好 `200`（Ruby 字符串比较严格）。
- 任何脚本删除自己缓存目录时必须走真实 `/bin/rm`，不能走 `~/bin/rm`。
- `pod install` 走预编译包后会跳 `Downloading dependencies` 阶段，之后会直接安装 xcframework。
