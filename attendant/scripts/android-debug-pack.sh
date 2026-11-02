#!/bin/sh
# Pack or check the OpenBot Android debug client for the attendant API.
#
# Guarantees:
# - check never writes, never starts Gradle or Metro, never installs.
# - pack and link refuse when GradleWrapperMain is already running.
# - A packaged native library is accepted only when its first four bytes are ELF.
# - Metro is not started when TCP 8081 is already listening.
#
# Does not:
# - clean the android tree
# - pass Gradle --build-cache
# - run ninja with more than one job
# - kill a process by name
# - embed the JS bundle (a debug dev-client still loads Metro)
#
# Modes: check | link | pack | repack | connect
# check is the default.

set -u

mode="${1:-check}"
root="${OPENBOT_MOBILE_ROOT:-/Volumes/codeMason/code/02-production/openbot/apps/mobile}"
android="$root/android"
# A stale ANDROID_HOME in the shell must not hide the SDK that actually built this APK.
if [ -n "${ANDROID_HOME:-}" ] && [ -d "$ANDROID_HOME" ]; then
  sdk="$ANDROID_HOME"
else
  sdk="/Volumes/codeMason/android-sdk"
fi
if [ -n "${JAVA_HOME:-}" ] && [ -x "${JAVA_HOME}/bin/java" ]; then
  java_home="$JAVA_HOME"
else
  java_home="/usr/local/Cellar/openjdk@21/21.0.7/libexec/openjdk.jdk/Contents/Home"
fi
gradle_user="${GRADLE_USER_HOME:-/Volumes/codeMason/gradle-user}"
work_tmp="${TMPDIR:-/Volumes/codeMason/tmp}"
api_url="${EXPO_PUBLIC_OPENBOT_API_URL:-https://api.attendant.mwcpool.uk}"
apk="$android/app/build/outputs/apk/debug/app-debug.apk"
adb="$sdk/platform-tools/adb"
ninja="$sdk/cmake/3.22.1/bin/ninja"
zipalign="$sdk/build-tools/37.0.0/zipalign"
apksigner="$sdk/build-tools/37.0.0/apksigner"
keystore="${ANDROID_DEBUG_KEYSTORE:-$HOME/.android/debug.keystore}"
out_apk="$work_tmp/openbot-mobile-elf.apk"

die() {
  echo "FAIL $*"
  exit 1
}

gradle_busy() {
  pgrep -f 'GradleWrapperMain' >/dev/null 2>&1
}

# Print name, size, magic. Exit 0 only for a real ELF.
elf_line() {
  python3 - "$1" << 'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
if not path.is_file():
    print(f"MISSING {path.name}")
    raise SystemExit(1)
data = path.read_bytes()
magic = data[:4]
ok = magic == b"\x7fELF" and len(data) > 1000
print(f"{'ELF' if ok else 'BAD'} {path.name} size={len(data)} magic={magic.hex()}")
raise SystemExit(0 if ok else 1)
PY
}

newest_obj() {
  python3 - "$android" << 'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
hits = sorted(root.glob("app/build/intermediates/cxx/Debug/*/obj/arm64-v8a"), key=lambda p: p.stat().st_mtime, reverse=True)
print(hits[0] if hits else "")
PY
}

scan_apk() {
  python3 - "$1" << 'PY'
import sys, zipfile
path = sys.argv[1]
try:
    z = zipfile.ZipFile(path)
except FileNotFoundError:
    print("APK_MISSING")
    raise SystemExit(1)
bad = 0
count = 0
for info in z.infolist():
    if not info.filename.endswith(".so"):
        continue
    count += 1
    magic = z.read(info.filename)[:4]
    if magic != b"\x7fELF":
        bad += 1
        print(f"BAD {info.filename} size={info.file_size} magic={magic.hex()}")
print(f"APK_SO count={count} bad={bad}")
raise SystemExit(1 if bad or count == 0 else 0)
PY
}

metro_bind() {
  lsof -nP -iTCP:8081 -sTCP:LISTEN 2>/dev/null || true
}

cmd_check() {
  echo "MODE check"
  echo "ROOT $root"
  echo "API $api_url"
  [ -d "$android" ] || die "android tree missing: $android"
  [ -x "$java_home/bin/java" ] || die "JDK 21 missing: $java_home"
  [ -d "$sdk" ] || die "ANDROID_HOME missing: $sdk"
  if gradle_busy; then
    echo "GRADLE busy — do not start another build"
  else
    echo "GRADLE idle"
  fi
  obj="$(newest_obj)"
  echo "OBJ ${obj:-none}"
  if [ -n "$obj" ]; then
    for name in libappmodules.so libreanimated.so libgesturehandler.so librnscreens.so; do
      if [ -f "$obj/$name" ]; then
        elf_line "$obj/$name" || true
      fi
    done
  fi
  if [ -f "$apk" ]; then
    echo "TREE_APK $apk"
    scan_apk "$apk" || true
  else
    echo "APK_MISSING $apk"
  fi
  if [ -f "$out_apk" ]; then
    echo "REPAIRED_APK $out_apk"
    scan_apk "$out_apk" || true
  fi
  echo "--- metro ---"
  metro_bind
  echo "--- adb ---"
  if [ -x "$adb" ]; then
    "$adb" devices -l
    serial="$("$adb" devices | awk '/\tdevice$/{print $1; exit}')"
    if [ -n "$serial" ]; then
      echo "REVERSE $serial"
      "$adb" -s "$serial" reverse --list || true
    fi
  fi
}

relink_one() {
  obj="$1"
  cxx_dir="$2"
  name="$3"
  target="${name#lib}"
  target="${target%.so}"
  if elf_line "$obj/$name"; then
    echo "KEEP $name"
    return 0
  fi
  [ -x "$ninja" ] || die "ninja missing: $ninja"
  [ -d "$cxx_dir" ] || die "ninja dir missing: $cxx_dir"
  echo "LINK $target -j1"
  "$ninja" -j1 -C "$cxx_dir" "$target" || die "ninja failed: $target"
  elf_line "$obj/$name" || die "still not ELF: $name"
}

cmd_link() {
  gradle_busy && die "Gradle is already running"
  obj="$(newest_obj)"
  [ -n "$obj" ] || die "no arm64 obj directory; prebuild first, do not clean"
  hash="$(basename "$(dirname "$(dirname "$obj")")")"
  cxx="$android/app/.cxx/Debug/$hash/arm64-v8a"
  export PATH="$work_tmp/bin:$PATH"
  mkdir -p "$work_tmp/bin"
  if [ ! -x "$work_tmp/bin/ccache" ]; then
    printf '%s\n' '#!/bin/sh' 'exec "$@"' > "$work_tmp/bin/ccache"
    chmod 755 "$work_tmp/bin/ccache"
  fi
  for name in libreact_codegen_RNKC.so libreact_codegen_rnscreens.so libreact_codegen_rnsvg.so libreact_codegen_rngesturehandler_codegen.so libreact_codegen_safeareacontext.so libappmodules.so; do
    relink_one "$obj" "$cxx" "$name"
  done
}

cmd_pack() {
  cmd_link
  gradle_busy && die "Gradle is already running"
  cd "$android" || die "cannot enter $android"
  export JAVA_HOME="$java_home"
  export ANDROID_HOME="$sdk"
  export ANDROID_SDK_ROOT="$sdk"
  export GRADLE_USER_HOME="$gradle_user"
  export TMPDIR="$work_tmp"
  export EXPO_PUBLIC_OPENBOT_API_URL="$api_url"
  export PATH="$work_tmp/bin:$PATH"
  ./gradlew app:assembleDebug \
    -x lint -x test \
    -PreactNativeArchitectures=arm64-v8a \
    -Dorg.gradle.java.home="$java_home" \
    -Dorg.gradle.parallel=false \
    || die "gradle failed"
  scan_apk "$apk" || die "apk contains a non-ELF .so — run repack, do not install this file"
  echo "PACK_OK $apk"
}

find_elf() {
  name="$1"
  obj="$(newest_obj)"
  if [ -n "$obj" ] && elf_line "$obj/$name" >/dev/null; then
    echo "$obj/$name"
    return 0
  fi
  python3 - "$root" "$name" << 'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
name = sys.argv[2]
hits = []
for path in root.rglob(name):
    if "obj/arm64-v8a" not in path.as_posix():
        continue
    try:
        magic = path.read_bytes()[:4]
    except OSError:
        continue
    if magic == b"\x7fELF" and path.stat().st_size > 1000:
        hits.append(path)
hits.sort(key=lambda p: p.stat().st_mtime, reverse=True)
print(hits[0] if hits else "")
PY
}

cmd_repack() {
  [ -f "$apk" ] || die "apk missing: $apk"
  [ -x "$zipalign" ] || die "zipalign missing"
  [ -x "$apksigner" ] || die "apksigner missing"
  [ -f "$keystore" ] || die "debug keystore missing"
  python3 - "$apk" "$out_apk.unaligned" "$android" "$root" << 'PY'
import pathlib, sys, zipfile
src, dst, android, mobile = sys.argv[1:]
need = {}
def elf(path):
    data = pathlib.Path(path).read_bytes()
    return data[:4] == b"\x7fELF" and len(data) > 1000
for name in (
    "libappmodules.so", "libexpo-modules-worklets.so", "libgesturehandler.so",
    "libreact_codegen_RNKC.so", "libreact_codegen_rngesturehandler_codegen.so",
    "libreact_codegen_rnscreens.so", "libreact_codegen_rnsvg.so",
    "libreact_codegen_safeareacontext.so", "libreanimated.so", "librnscreens.so",
):
    hits = []
    for base in (pathlib.Path(android), pathlib.Path(mobile).parent.parent / "node_modules"):
        if not base.exists():
            continue
        for path in base.rglob(name):
            if "obj/arm64-v8a" not in path.as_posix():
                continue
            if elf(path):
                hits.append(path)
    hits.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    if not hits:
        raise SystemExit(f"NO_ELF {name}")
    need["lib/arm64-v8a/" + name] = hits[0]
    print(f"USE {name} {hits[0]}")
with zipfile.ZipFile(src) as zin, zipfile.ZipFile(dst, "w") as zout:
    for item in zin.infolist():
        if item.filename in need:
            info = zipfile.ZipInfo(filename=item.filename, date_time=item.date_time)
            info.compress_type = zipfile.ZIP_STORED
            info.external_attr = item.external_attr
            zout.writestr(info, need[item.filename].read_bytes())
        else:
            zout.writestr(item, zin.read(item.filename))
print("REPACK_WRITTEN", dst)
PY
  "$zipalign" -p -f 4 "$out_apk.unaligned" "$out_apk.aligned"
  "$apksigner" sign \
    --ks "$keystore" \
    --ks-key-alias androiddebugkey \
    --ks-pass pass:android \
    --key-pass pass:android \
    --out "$out_apk" \
    "$out_apk.aligned"
  "$apksigner" verify "$out_apk" || die "signature failed"
  scan_apk "$out_apk" || die "repacked apk still has a bad .so"
  echo "REPACK_OK $out_apk"
}

cmd_connect() {
  [ -x "$adb" ] || die "adb missing"
  serial="${ADB_SERIAL:-}"
  if [ -z "$serial" ]; then
    serial="$("$adb" devices | awk '/\tdevice$/{print $1; exit}')"
  fi
  [ -n "$serial" ] || die "no adb device"
  if ! lsof -nP -iTCP:8081 -sTCP:LISTEN >/dev/null 2>&1; then
    die "Metro is not listening on 8081. Start one with: cd apps/mobile && EXPO_PUBLIC_OPENBOT_API_URL=$api_url bun expo start --dev-client --host lan --port 8081"
  fi
  ipv4="$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 http://127.0.0.1:8081/status || true)"
  echo "METRO_IPV4 $ipv4"
  [ "$ipv4" = "200" ] || die "127.0.0.1:8081 is not Metro. It may be IPv6-only. Restart Metro with --host lan. Do not start a second Metro."
  "$adb" -s "$serial" reverse tcp:8081 tcp:8081
  "$adb" -s "$serial" reverse --list
  "$adb" -s "$serial" shell am start -a android.intent.action.VIEW -d "openbot://expo-development-client/?url=http%3A%2F%2F127.0.0.1%3A8081"
  echo "CONNECT_OK $serial"
}

case "$mode" in
  check) cmd_check ;;
  link) cmd_link ;;
  pack) cmd_pack ;;
  repack) cmd_repack ;;
  connect) cmd_connect ;;
  *) die "mode must be check, link, pack, repack, or connect" ;;
esac
