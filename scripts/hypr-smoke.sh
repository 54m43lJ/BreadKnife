#!/usr/bin/env bash
# Hyprland 接口冒烟（真机会话内执行）：环境与套接字可达、hyprctl JSON 数据源、
# dispatch 语义、socket2 事件流。
# 不改会话状态：dispatch 仅在当前工作区名 / id 上原位切换（无视觉副作用）；
# 相对切换 m+1/m-1 会真实换工作区再切回，可用 SKIP_DISPATCH=1 跳过。
set -u

pass=0
failn=0
ok()   { echo "  ok: $1"; pass=$((pass + 1)); }
fail() { echo "FAIL: $1"; failn=$((failn + 1)); }

command -v hyprctl >/dev/null 2>&1 || { echo "FAIL: hyprctl 不在 PATH"; exit 1; }
: "${HYPRLAND_INSTANCE_SIGNATURE:?FAIL: HYPRLAND_INSTANCE_SIGNATURE 未设置}"
runtime="${XDG_RUNTIME_DIR:-/run/user/$UID}"
sock="$runtime/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock"
if [ -S "$sock" ]; then ok "socket2 套接字可达 ($sock)"; else fail "socket2 套接字缺失: $sock"; fi

# ── hyprctl JSON 数据源（daemon 的三个事实源）──
for cmd in monitors workspaces clients activeworkspace activewindow; do
    hyprctl -j "$cmd" >/dev/null 2>&1 && ok "hyprctl -j $cmd" || fail "hyprctl -j $cmd"
done

# ── dispatch 语义（Hyprland ≥0.55 走 Lua hl.dsp.focus；旧版回退经典 workspace 语法）──
lua_ws()    { hyprctl dispatch "hl.dsp.focus({ workspace = \"$1\" })" >/dev/null 2>&1; }
legacy_ws() { hyprctl dispatch workspace "$1" >/dev/null 2>&1; }
ws_switch() { lua_ws "$1" || legacy_ws "$1"; }

cur="$(hyprctl -j activeworkspace 2>/dev/null)"
name="$(printf '%s' "$cur" | grep -o '"name": *"[^"]*"' | head -1 | cut -d'"' -f4)"
id="$(printf '%s' "$cur" | grep -o '"id": *[0-9]*' | head -1 | tr -dc '0-9')"
if [ -n "$name" ]; then
    if lua_ws "$name"; then
        ok "dispatch Lua（hl.dsp.focus）可用"
    elif legacy_ws "$name"; then
        ok "经典 dispatch 语法可用（当前 Hyprland 不认 Lua dispatch）"
    else
        fail "dispatch 两种语法均不可用"
    fi
    ws_switch "$name" >/dev/null 2>&1 \
        && ok "dispatch workspace <name=$name> 原位切换" || fail "dispatch workspace name"
else
    fail "activeworkspace 无 name 字段，无法验证按名切换"
fi
if [ -n "$id" ]; then
    ws_switch "$id" >/dev/null 2>&1 \
        && ok "dispatch workspace <id=$id> 原位切换" || fail "dispatch workspace id"
fi
if [ "${SKIP_DISPATCH:-0}" != "1" ]; then
    if ws_switch m+1 && ws_switch m-1; then
        ok "dispatch 相对切换 m+1/m-1（往返，条会闪一下）"
    else
        fail "dispatch 相对切换"
    fi
fi

# ── socket2 事件流：监听 3 秒，触发一次 configreloaded，统计收到的事件 ──
tmp="$(mktemp)"
python3 - "$sock" >"$tmp" <<'EOF' &
import socket
import sys

s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sys.argv[1])
s.settimeout(3.0)
buf = b""
try:
    while True:
        d = s.recv(4096)
        if not d:
            break
        buf += d
except socket.timeout:
    pass
sys.stdout.write(buf.decode(errors="replace"))
EOF
py=$!
sleep 0.3
hyprctl reload >/dev/null 2>&1
wait "$py" 2>/dev/null
lines=$(grep -c . "$tmp" || true)
if [ "$lines" -gt 0 ]; then
    ok "socket2 事件流收到 $lines 行"
else
    fail "socket2 3 秒内未收到任何事件"
fi
if grep -q '^configreloaded' "$tmp"; then
    ok "socket2 收到 configreloaded（daemon 全量重同步的触发源）"
else
    fail "未收到 configreloaded"
fi
rm -f "$tmp"

echo "----"
echo "通过 $pass 项，失败 $failn 项"
[ "$failn" -eq 0 ]
