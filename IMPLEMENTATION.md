# BreadKnife 实现记录

> 定位：`dev/breadknife-breadpan` 交付的实现事实与实现笔记。
> 权威顺序：**代码 > 本文件 > 四份 HTML 规格**——本文件与代码不一致时以代码为准。
> 四份 HTML 文档（[README](README.html) / [ARCHITECTURE](ARCHITECTURE.html) / [CODE](CODE.html) / [BUILD](BUILD.html)）
> 是面向使用方的规格；按文档规约，工程取值（时长、周期、容量、阈值）与具体取舍只记在本文件，四文档不重复。
> 对齐标记：`✓` 表示该行为已与所列文档章节对齐；无标记的条目为实现笔记（四文档按规约不收录）。
> 本次收尾同时归档删除了五份 md 工作稿（BOUNDARY / INTEGRATION / MIGRATION / ISSUES / GUIDELINES），
> 其内容状态已在交付报告中逐项声明（全部为「实现 / 调整」，无未决项）。

## 1. 交付范围

单一二进制 `bread`（版本 0.1.0，edition 2024），同一程序承载三种角色：daemon、窗口进程、控制客户端。
本交付相对 main 的实现范围：

- 运行模型重构：单进程 astal 任务栏 → **daemon（Model，无 GTK）+ 每显示器窗口进程（View，GTK4）** 双进程架构，内部 Unix Socket IPC；
- 托盘协议栈替换：`astal-tray` → 仓库内成员库 **breadpan**（SNI + DBusMenu 单库，进程级组装）；
- 交互完善：工作区四要素矩阵、持久紧急语义、经典嵌套式托盘菜单（macOS 外观 + 悬停跟踪）、弹窗会话统一托管；
- 工程面：纯 Rust 依赖（无 libastal / appmenu-glib-translator 系统依赖）、CI 迁移、Hyprland 接口冒烟脚本。

模块与规格出处：

| 模块 | 职责 | 代码 | 规格出处 |
|---|---|---|---|
| 入口 / 客户端 | 子命令分发、后台化、控制行转发 | `src/main.rs` | ✓ README「控制指令 / 运行管理」、BUILD「运行」 |
| daemon 装配与事件循环 | 事件汇聚、重同步、窗口生命周期、停机 | `src/daemon/mod.rs` | ✓ ARCHITECTURE §3.3、§4.2；CODE §4.1–4.3 |
| Hyprland 集成 | socket2 只读监听、hyprctl 事实源、工作区切换 | `src/daemon/hyprland.rs` | ✓ ARCHITECTURE §4.1；README「界面交互」 |
| 托盘服务 | breadpan 组装、事件泵、交互与菜单读取 | `src/daemon/tray.rs` | ✓ ARCHITECTURE §4.3；README「系统托盘」 |
| 系统服务 / 通知 | UPower / NetworkManager / BlueZ / MPRIS、通知守护、调节后端 | `src/daemon/system.rs`、`src/daemon/notify.rs` | ✓ ARCHITECTURE §3.2/§4.5 |
| daemon IPC | 控制 socket（行协议）+ 窗口 socket（帧协议） | `src/daemon/ipc.rs` | ✓ ARCHITECTURE §4.2/§4.4 |
| 窗口进程 | 层壳窗口、模块装配、数据分发 | `src/bar/mod.rs` | ✓ ARCHITECTURE §3.3/§3.10 |
| 窗口 IPC 客户端 | 快照 / 推送订阅、请求投递与重连 | `src/bar/ipc.rs` | ✓ ARCHITECTURE §4.4（§4.4 note：断连处理） |
| 工作区条 | 四要素渲染 + 手势 | `src/bar/modules/workspace.rs` | ✓ README「界面交互」、ARCHITECTURE §3.10 |
| 托盘 UI / 菜单 | 图标直出、手势、自绘菜单、悬停跟踪 | `src/bar/modules/tray.rs` | ✓ README「系统托盘 / 菜单交互」、ARCHITECTURE §4.4 |
| 弹窗会话 | 统一打开 / 定位 / 关闭、悬停切换轮询 | `src/bar/modules/popup.rs` | ✓ README「菜单交互」、ARCHITECTURE §4.4 |
| 窗口信息 / 通知中心 | 标题、通知列表弹窗 | `src/bar/modules/window_info.rs` | ✓ README「窗口信息 / 通知中心」 |
| 控制中心 | 入口（电池/网络/时钟）与弹出面板 | `src/bar/modules/center.rs` | ✓ README「控制中心」、ARCHITECTURE §3.10 |
| 共享协议 / 模型 | 请求 / 应答 / 推送、共享数据模型 | `src/shared/protocol.rs`、`src/shared/model.rs` | ✓ ARCHITECTURE §4.4/§4.5 |
| 默认主题 | 调色板与组件样式 | `src/assets/bread.css` | ✓ ARCHITECTURE §3.9/§3.10 |

## 2. 运行模型与进程架构

- `bread` 前台启动 daemon；`bread daemon` 以 `__daemon` 子命令后台化（stdout 丢弃，stderr 保留到启动终端）。
- daemon 启动前校验 PID 文件（`$XDG_RUNTIME_DIR/bread.pid`），进程存活则拒绝二次启动；随后写入自身 PID。
- 控制 socket 默认 `$XDG_RUNTIME_DIR/bread.sock`，可用 `-s/--socket` 覆盖；窗口 socket 由控制 socket 派生（同目录、`<stem>-windows.sock`）。
- 显示器 → 窗口映射由 daemon 持有：启动首轮重同步把显示器清单种入 `desired` 集合并逐屏 spawn；此后 `desired` 只由 `open` / `close` / 显示器插拔事件变更（用户关闭的窗口不因重同步复活）。
- 窗口进程由 daemon spawn：`bread __window --monitor <name> --socket <window-sock>`；kill 路径置 `killing` 标记、断开下行通道、`kill()` 后由独立线程 `wait()` 收割，避免僵尸残留。
- 崩溃重启策略：60s 滑动窗口内最多 5 次，超限放弃并从 `desired` 移除；每次重启前延时 1s。子进程退出经轮询（300ms）回收进事件循环。
- 窗口侧承载三层守护：daemon 主动终止（exit / close / 显示器移除）；daemon 存活周期检查（2s，PID 文件 + `/proc`）确认已退出则自主退出；互动失败时请求路径重连一次，仍失败则按上述存活检查处理。
- 事件循环以 200ms 滴答为超时档；Hyprland 事件风暴经 `pending_resync` 去抖（滴答窗口内合并为一次全量重读）。
- 窗口进程启动时若未设置 `GSK_RENDERER`，自动回退 `cairo`（VMware/vmwgfx 的 dmabuf 导入不可用，规避 Wayland dmabuf 协议错误崩溃）。

## 3. daemon：Hyprland 集成

- 事实源：`hyprctl -j monitors / workspaces / clients / activeworkspace / activewindow` 与 `hyprctl -j workspacerules`（工作区规则）。
- 事件面：`$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock` 只读监听；连接失败每秒重试，共 10 次后放弃（降级为无事件驱动）。
- 消费的事件：
  - `monitoradded(v2)` / `monitorremoved(v2)` → 维护 `desired`，触发重同步；
  - `activewindow`（v1 载荷：class,title）→ 活动窗口类/标题；`activewindowv2`（0.56 载荷：仅窗口地址，可为空）→ 只记录地址；
  - `windowtitlev2` → 地址命中当前活动窗口时更新标题；
  - `urgent` / `urgentdecode` → 地址反查工作区并**即时置位**（不等下一次重同步）；
  - `workspace(v2)` / `focusedmon(v2)` / `openwindow` / `closewindow` / `movewindow(v2)` / `createworkspace(v2)` / `destroyworkspace(v2)` / `changefloatingmode` / `configreloaded` / `activespecialworkspace` → 合并触发全量重同步。
- 地址归一：事件载荷不带 `0x` 前缀、`hyprctl -j clients` 带前缀；统一去前缀并小写后比较。
- 工作区切换：优先 Hyprland ≥0.55（Lua 配置）的 `hl.dsp.focus({ workspace = "…" })`，失败回退经典 `dispatch workspace <target>`（含 Lua 字符串转义）。
- 工作区切片（每屏）＝本屏现存工作区 ∪ 绑定到本屏的启用规则：
  - 规则选择器仅接受静态形式（数值 id、`name:<名>`）；`special:*`、`empty*` 等动态选择器不产生常驻条目；
  - 按名对全量现存工作区去重（工作区移动到其它屏时不造幽灵）；按 id 排序；
  - 仅规则、尚未创建的工作区：数值规则用其 id；命名规则取合成负 id（自 `-100000` 起递减）；
  - 仅规则条目＝非活跃工作区：无窗口、不活跃、不紧急。
- 紧急（urgent）语义——持久锁存：
  - 置位：仅 `urgent` 事件；置位后立即重建并推送切片；
  - 清除路径①：切换到该工作区（按显示器比较活跃工作区，发生跃迁才清除；首次见屏只记基线，同工作区重复重同步不清除——活跃期间迟到的 urgent 得以保留）；
  - 清除路径②：该工作区活跃时点击它（`workspace.switch` 按工作区名精确解析；`m±1` 等相对目标不按名解析，交由跃迁路径清除）；
  - 工作区销毁后清掉幽灵位，防止复用时旧紧急位复活。
- ✓ ARCHITECTURE §4.1（事件契约）、§4.5（工作区映射与重建）；✓ README「界面交互」（矩阵、非活跃工作区、紧急语义）。

## 4. daemon：托盘服务（breadpan）

- 组装：进程内 `Tray::start(TrayConfig::default())` 一次完成；失败（含 `NameTaken`）降级为空托盘并记日志，不阻断其余功能。
- 事件泵：库内 Receiver 唯一，专用线程把 `TrayEvent` 转投 daemon 事件面。
- 事件处理：
  - `Snapshot` → 重建注册表并逐项按 `Added` 广播（窗口通常尚未接入，幂等补投）；
  - `Added` / `Changed` → 在专用线程现场重读 `item(id)`（阻塞 2–4s，不得卡事件泵），读回后按是否新项分别推送 `Added` / `Changed`；读取失败仅日志；
  - `Removed` → 出注册表并广播移除；
  - `MenuChanged` → 广播 `menu_changed`（仅打开该 Item 菜单的窗口重取）；
  - `NameChanged` → 重键：旧 id 广播移除、新 id 现场重读（对象生死不变，不再出两张图标）；
  - `OwnerChanged` → 仅日志（信息性）；`InteractionResult` 失败 → 日志；`Narration` → 日志；
  - `Fatal` / `Displaced` → **终局降级**：丢弃句柄、清空注册表并广播移除；不自动重连、不夺回（库语义即终局）。
- 交互执行：左 / 中 / 右 / 四向滚轮（垂直 + 水平，步数聚合）全部经 breadpan `*_click_at` / `scroll_*_by`；菜单读取 `menu()`、子层 `menu_expand()`、激活 `menu_activate()`。所有托盘调用在 daemon 的专用线程执行（每请求一线程），同步阻塞只发生在其调用线程。
- 库侧修正（本交付）：`Tray::start` 引导总超时分支原先先 `join()` 后释放就绪通道，病态引导（>20s）下运行线程就绪发送成功后驻停，`join` 无限等待；现改为**先释放就绪通道再 join**，超时错误可正常返回（`breadpan/src/handle.rs`）。
- ✓ ARCHITECTURE §4.3（组装 / 封装边界 / 执行位置 / 驱动源）；✓ README「系统托盘」；✓ BUILD「调试要点」。

## 5. daemon：系统服务与通知

- 系统状态：UPower（电池）、NetworkManager（网络）、BlueZ（蓝牙）、MPRIS（媒体）经 zbus 消费；状态变化才广播（`system` 增量）。
- 调节后端：`pamixer`（音量）、`brightnessctl`（内置面板亮度）、`ddcutil`（外接显示器 DDC/CI）；缺席时对应功能灰置降级。
- 通知守护：实现 `org.freedesktop.Notifications`；记录应用名、图标、摘要、正文、通知 ID；同 ID 更新、异 ID 入队；保留上限 50 条；广播全部窗口。
- 媒体 / 音量 / 亮度操作均在专用线程执行，主循环只投递受理应答。
- ✓ ARCHITECTURE §3.2（技术栈）、§4.5（通知记录）；✓ README「控制中心 / 通知中心」。

## 6. IPC（协议与断连）

- 帧格式：4 字节小端长度 + JSON（`Frame::{Request, Response, Push}`，请求/应答带关联 id，推送 id 恒 0）；单帧上限 16 MiB（位图图标经 base64 跨 JSON 的护栏）。
- 控制接口：文本行协议，一行一条；解析为 `open/close`（可带 `-m <monitor>`）、`open-all`、`close-all`、`exit`、`reload`；处理结果不回写。
- 窗口接口：每连接一个读线程（请求上行）+ 一个写线程（应答与推送共用，串行化避免帧交错）；`Hello { monitor }` 握手先推全量快照、后回应答。
- 客户端请求在专用线程阻塞往返（35s 超时），失败重连一次后重试；重连按首连流程再取全量快照。
- 控制指令语义：`open` / `close` 只改 `desired` 集合并触发重同步（窗口名为 `bar`）；`open-all` / `close-all` 作用于全部显示器；`reload` 广播 `Push::Reload`；`exit` 终止全部窗口、`TrayHandle::shutdown()` 释放 Watcher 名、删除 PID 与两个 socket 后进程退出。
- ✓ ARCHITECTURE §4.2（控制接口）、§4.4（daemon ↔ 窗口契约；线格式明示为实现细节）。

## 7. 窗口进程与模块装配

- GTK Application 以 `NON_UNIQUE` 运行（每个显示器一个实例）；层壳窗口：layer `Top`，锚定 `Top|Left|Right`，namespace `bread`，自动独占区；高度 30px，宽度跟随显示器几何并订阅几何变化（分辨率变更）。
- 装配顺序：IPC 连接（Hello 握手）→ 层壳窗口 → `PopupManager::install`（窗口级关闭面先于模块）→ 工作区 / 窗口信息 / 托盘 / 控制中心 → `CenterBox` 布局（工作区居左、标题居中、托盘与控制中心居右收尾）→ present。
- UI 任务队列（std mpsc）统一承载推送与异步回执，GTK 主线程每 40ms 排空一次；请求一律在工作线程往返。
- 快照应用：工作区、活动窗口、托盘项、系统状态、通知缓存整体重建；CSS 路径变化时重读主题。
- ✓ ARCHITECTURE §3.3（运行模型）、§3.10（组件样式）；✓ CODE §4.1.2 / §4.2。

## 8. 工作区条

- 每屏一条；容器间距 2px；每个工作区一个按钮，四要素独立决定呈现：
  - 基底形态＝有窗口长方形（20×15px，圆角 10px）／无窗口小圆点（直径 4px）；
  - 基底色＝紧急 `warning`（叠加点取背景色保证对比）／否则 `fg_muted`；
  - 叠加形态＝活跃小圆点（直径 4px；与基底圆点等大重叠，无窗口时视觉为单个 `fg_primary` 小圆点）；
  - 叠加色＝紧急基底取背景色／否则 `fg_primary`。
- 悬停提示 `workspace <name>`；点击按工作区名发起切换；滚轮上＝左侧工作区、下＝右侧，滚轮左右自然对应（相对目标 `m-1` / `m+1`）。
- 渲染使用 CSS 类：`ws-btn`、`ws-base.square|dot`、`urgent`、`ws-ind`、`on-warning`；叠加点用 `GtkOverlay` 保持契约尺寸。
- ✓ README「界面交互」（矩阵 + 说明）；✓ ARCHITECTURE §3.10（色彩角色与尺寸契约，操作源在 `bread.css`）。

## 9. 托盘 UI 与菜单

图标层：

- 图标 16px，经 `breadpan::gtk::icon_paintable` 渲染直出（主题名 / 文件路径 / 位图三态）。
- 自动垫高对比底色：像素 alpha 加权平均亮度采样（≤512 采样点，阈值 0.55），深色图标加深底色、浅色图标加浅底色；不可采样时不加。
- 失联（alive=false）置灰不移除；attention 状态单独样式；悬停显示 Tooltip。
- 手势：左 / 中 / 右键 → `tray.interact`（含部件局部坐标）；四向滚轮按主轴分流（垂直优先，Shift+滚轮在 GTK 侧表现为水平增量），步数 1。
- 弹窗抓取期间同一图标的按下先被捕获阶段关闭自己的菜单，随后跳过本次交互（toggle 语义）。

菜单层（经典嵌套式 + macOS 外观）：

- 根菜单：无箭头气泡（`has_arrow(false)`）、平铺面板；贴任务栏下缘（popover 定位到图标矩形，`Bottom`/`Fill`）。
- 子菜单：嵌套 popover（父 = 上级 popover），锚定该行右侧、顶边与行对齐；锚点矩形左右外扩 5px（父面板内衬 padding 4px + border 1px），实现与父面板边缘相接的贴边对齐；无箭头修饰。
- 行级悬停统一规则（根行与子行同规则）：进入行 R → 收起所有「锚点不是 R 且不含 R」的子层；回到父条目、菜单空白或移出菜单均保留；指针离开菜单不关闭。
- 悬停即展开（含 item / menu / submenu）：有子层且可用的行，驻留 150ms 即展开；掠过不触发（进入/离开递增世代令牌，超时回调校验令牌）。同锚子层已在显示时不重复展开（悬停与点击竞态）。
- 点击：子层行 → 懒加载展开；叶子行 → `menu_activate`（受理即返回）后由回执关闭菜单。
- 渲染细节：toggle 忠实透传（✓ / ▬ / ● / ◍，无乐观更新）；助记符交由 GTK 下划线解析；子层箭头 ▸。
- 刷新：`MenuChanged` 推送 → 已打开该 Item 菜单的窗口重取根层并重建（已展开子层先收，用户按需再展开）。
- 外观：`.menu-popover` 平铺面板，无气泡尾与描边环；`contents` 节点内衬 4px + 1px 边框。

菜单跟踪（悬停切换托盘图标，macOS 菜单栏式）：

- 弹窗打开期间，任务栏窗口收不到 motion 事件（合成器把指针焦点钉在 popup 表面，popup grab 的平台行为），故由**合成器指针轮询**驱动：读 Hyprland 命令套接字 `j/cursorpos`，周期 50ms；仅弹窗打开期间轮询（全部关闭即收停）。
- 命中锚点（托盘图标按窗口坐标做边界判定）后驻留 150ms 触发切换：读该图标菜单并替换当前菜单；世代令牌校验异步回执（离开或换锚即作废）。
- 无弹窗显示时悬停不产生任何动作；掠过不触发。

✓ README「系统托盘 / 菜单交互」（三条说明：经典嵌套、macOS 外观、菜单跟踪）；✓ ARCHITECTURE §4.4（`menu_changed` 定向推送）。

## 10. 控制中心与通知中心

- 入口：电池（无电池设备隐藏）、网络状态、实时时钟。
- 控制中心弹窗：媒体控制（播放/暂停、上/下一曲）、音量与亮度滑块、网络 / 蓝牙状态、日历；滑块经 200ms 节流下发（值变化才发）。
- 右对齐：弹窗挂根窗口，锚点为右端容器，`halign=End` + 锚点矩形定位，贴紧任务栏右缘（不参与容器布局，不会撑宽右对齐元素）。
- 通知中心：点击标题区域弹出通知列表，保留最近 20 条；新通知到达时刷新。
- 弹窗会话统一由 `PopupManager` 托管（组标签 `cc` / `notif` / `tray:<item>`），任务栏内按下无缝切换、外部按下由 grab 收回并吞掉、Escape 兜底。
- ✓ README「窗口信息 / 通知中心 / 控制中心」；✓ ARCHITECTURE §4.4。
- 说明：控制中心 / 通知中心不做悬停切换（验收时明示不在范围内）。

## 11. 样式系统

- 默认主题编译期内嵌（`include_str!`），首次启动落盘到 `$XDG_RUNTIME_DIR/bread/bread.css`；`BREAD_CSS` 环境变量可整体覆盖路径；daemon 跟踪该路径并随快照下发。
- `bread reload` → `Push::Reload` → 各窗口按跟踪路径重读 CSS（改色即见）。
- 调色板定义未改动（`bg_active` 保持 `#3d3835`）；工作区用色全部取既有色板：基底 `fg_muted #6b5d48`、前景 `fg_primary`、紧急 `warning`。
- ✓ ARCHITECTURE §3.9（变量集）、§3.10（组件样式与色彩角色）；✓ README「个性化」。

## 12. 构建 / 测试 / CI

- 依赖：`breadpan`（路径依赖，纯 Rust）、`gtk4`（仅窗口进程链接）、`gtk4-layer-shell`、`zbus`（tokio）、`tokio`、`serde`/`serde_json`、`base64`、`clap`；无 astal / libastal / appmenu-glib-translator。
- 应用测试（`cargo test`）：21 项——IPC 帧回环、控制行解析、Hyprland 事件 fixture、工作区切片与紧急清除语义等。
- 库测试：`dbus-run-session -- cargo test --manifest-path breadpan/Cargo.toml`（全量 mock）。
- Hyprland 接口冒烟：`scripts/hypr-smoke.sh`（12 项：socket2 可达、`hyprctl -j` 五个事实源、dispatch 双语法、socket2 事件流）。
- CI（`.github/workflows/preview-release.yml`）：已删除 libastal / appmenu-glib-translator 构建步骤；执行 breadpan 库测试与应用测试，构建并发布 `bread` 二进制。
- ✓ BUILD.html（依赖表 / 编译与测试 / 运行 / 调试要点 / 冒烟脚本）。

## 13. 与 HTML 文档的对齐核对

核对方式：以代码为权威逐条比对四份应用文档与 breadpan 成员文档；`✓` 为已对齐，`⚠` 为待更新（明细见 §13.1）。

| 文档 | 结论 | 说明 |
|---|---|---|
| README.html | ✓ 已对齐 | 界面交互表、工作区矩阵与三条说明、菜单交互三条说明、控制指令、个性化、运行管理、明确不做，全部与实现一致。 |
| ARCHITECTURE.html | ✓ 已对齐 | §3.2–§3.10、§4.1–§4.5 与实现一致；§4.1 为契约子集，实现消费的事件为超集（另有 `activewindowv2` / `windowtitlev2` / `focusedmon` / `openwindow` / `closewindow` / `movewindow` 等触发重同步，不影响契约）；§4.4 线格式（4 字节长度 + JSON）为文档明示的实现细节。 |
| CODE.html | ⚠ 部分对齐 | §1 文件树 ✓（仅缺 `src/tests.rs` 一行）；§2 为空 ✓（规约允许）；§3 多数条目仍是设计期草稿（带「待实现」标注，名称/签名与实现不同）；§4 流程骨架 ✓，若干调用的函数名与顺序与实现有差异。差异清单见 §13.1。 |
| BUILD.html | ✓ 已对齐 | 依赖 / 编译与测试 / 运行 / 运行前提 / 调试要点与实现一致；原两处对 md 工作稿的引用已就地改写为自含表述（本次收尾）。 |
| breadpan/README、ARCHITECTURE、CODE、BUILD | ✓ 抽检对齐 | CODE §3 文档化符号 18/18 均存在于源码；构建 / 测试命令与 CI 一致。本轮未做全文逐条复核（库文档在其自身交付线定稿）。 |
| reference/ | — | 外部规范存档（Hyprland / SNI / 图标规范），非项目文档。 |

### 13.1 CODE.html 待更新明细

按文档规约（实现已存在 → 使用真实名称、去掉「待实现」标注），以下条目需要按代码回填：

1. **§3.1 daemon/mod.rs**：实际入口 `pub fn run(control_path: &str) -> Result<(), String>`；`Daemon` 方法为 `run_loop` / `handle(DaemonEvent) -> bool` / `on_hyprland` / `resync` / `rebuild_workspace_slices` / `push_workspace_slices` / `spawn_window` / `kill_window` / `on_child_exited` / `on_tray` / `reread_tray_view` / `degrade_tray` / `on_window_request(u64, Request, Sender<Frame>)` / `exec_tray` / `on_control` / `open_windows` / `snapshot_for` / `push_to` / `broadcast` / `shutdown`。草稿中的 `start` / `on_hyprland_event` / `on_window_exit` / `push(window, …)` 不存在。
2. **§3.2 daemon/hyprland.rs**：实际类型 `MonitorInfo` / `WorkspaceRef` / `WorkspaceInfo` / `WorkspaceRuleInfo` / `ClientInfo`；函数 `monitors` / `workspaces` / `clients` / `workspace_rules` / `rule_workspace` / `normalize_addr` / `workspace_of_client` / `dispatch_workspace(target)` / `watch(Sender<DaemonEvent>)`。草稿 `MonitorId(String)` 不存在；`HyprlandEvent` 变体为 `MonitorAdded/Removed`、`ActiveWindow{class,title}`、`ActiveWindowAddr`、`WindowTitle{addr,title}`、`Urgent{addr}`、`Resync`。
3. **§3.3 daemon/notify.rs**：实际 `pub async fn serve(conn: &zbus::Connection, tx: Sender<DaemonEvent>) -> Result<(), zbus::Error>`。
4. **§3.4 daemon/ipc.rs**：实际 `serve_control(path: &str, tx) -> io::Result<()>`、`serve_windows(path, tx) -> io::Result<()>`、`reply(out, id, Response)`、`push(out, Push)`、`hello_reply(out, id, WindowSnapshot)`。草稿 `IpcServer` / `listen` 不存在。
5. **§3.5 bar/mod.rs**：实际 `pub fn run(monitor: &str, socket: &Path) -> Result<(), String>`；`WindowApp::create(app, monitor, socket) -> Result<Rc<Self>, String>`，另有 `handle_task` / `handle_push` / `apply_snapshot` 与 `UiTask` 枚举。草稿 `BarApp` 不存在。
6. **§3.6 bar/ipc.rs**：实际 `IpcClient::connect(path, monitor, ui_tx) -> Result<Arc<Self>, String>`、`request(&self, Request) -> Result<Response, String>`；`daemon_alive()` 位于 `shared/protocol.rs`。
7. **§3.7 bar/style.rs**：实际 `load(path: &str) -> Result<(), String>`、`reload(path: &str) -> Result<(), String>`（入参为 `&str`，错误为 `String`）。
8. **§3.8 bar/modules/tray.rs**：实际 `TrayWidget::new(ipc: Arc<IpcClient>, ui: Sender<UiTask>, window: &gtk::Window, popup: Rc<PopupManager>) -> Self`，公开面为 `container` / `rebuild` / `apply` / `menu_changed` / `on_interact_done` / `on_expand_done` / `on_activate_done` / `on_menu_read_done` / `on_hover_menu_done`；`show_menu` 为内部函数。
9. **§3.9 shared/protocol.rs**：消息类型之外新增 `Frame`（请求 / 应答 / 推送的统一帧）与 `ControlCommand`、线性编解码、路径与存活探测辅助；`Request` 实际含 `Hello`。消息语义仍以 ARCHITECTURE §4.4 为准。
10. **§3.10 shared/model.rs**：实际 `WorkspaceState { id: i64, name: String, urgent, active, has_windows }`（草稿 `id: u32` 且缺 `name`）；`ActiveWindow { class, title }`；另有 `IconView`、`TrayItemView`、`MenuSnapshot` / `MenuItem` / `MenuItemType` / `ToggleType` / `ToggleState`、`SystemState` / `BatteryState` / `NetworkState` / `BluetoothState` / `VolumeState`、`NotificationRecord`、`WindowSnapshot`、`StateDelta`；草稿 `NotificationRecord::from` 不存在。
11. **§4.1.1 启动**：实际顺序为 PID 文件 → `hyprland::watch` → `tray::start` → `serve_control` → `serve_windows` → `system::start` → `resync`（种入显示器并逐屏 spawn）→ 事件循环；草稿中的 `EventBus::start` / `ipc::listen` 名称不存在。
12. **§4.1.2 窗口进程**：CSS 不在启动前加载，而是随首帧快照下发后加载；另有 UI 任务排空定时器（40ms）与 daemon 存活周期检查（2s）。
13. **§4.2.1 / §4.2.2**：显示器增加 → `desired` 插入 + 重同步 spawn；快照在 `Hello` 握手后推送。`kill_window` 另有独立线程收割子进程。
14. **§4.2.3**：实际为「请求失败 → 重连一次重试；daemon 存活由 2s 周期检查判定」；互动失败并非唯一触发点。
15. **§4.2.4**：实际重启策略为 60s 内最多 5 次、每次延时 1s，超限放弃并移出 `desired`。
16. **§4.3 控制指令**：`open` / `close` 只操作 `desired`；`reload` 广播 `Push::Reload`；`exit` 终止窗口 + `TrayHandle::shutdown()` + 删除运行时文件后退出。
17. **§4.4 托盘右键**：实际还包含左键/中键/四向滚轮接线；菜单展开为 `TrayMenuExpand`、激活为 `TrayMenuActivate`、刷新为 `MenuChanged` → 根层重取；菜单为自绘 popover（含子层嵌套与悬停跟踪）。
18. **§4.5 通知到达**：同 ID 更新、异 ID 入队、上限 50 条，然后广播；窗口列表保留 20 条。

## 14. 验证状态与已知边界

- 构建 / 测试：`cargo test` 21/21；`dbus-run-session -- cargo test --manifest-path breadpan/Cargo.toml` 全量通过；`scripts/hypr-smoke.sh` 12/12。
- 真机（Hyprland 0.56.2）：启动 / 退出 / 工作区切换（Lua dispatch 与回退）/ 工作区矩阵与紧急 / 托盘图标增删改 / 左中右键与滚轮 / 菜单展开与激活 / 菜单跟踪悬停切换 / 控制中心与通知，均已在本会话中实际验证。
- 边界一：开发环境为单显示器（DP-2）——多显示器插拔矩阵（随插拔 spawn/kill、切片按屏）未在本环境实测。
- 边界二：无外接显示器——`ddcutil` 外接亮度路径未实测（内置 `brightnessctl` 路径已验证）。
- 边界三：特殊工作区（`special:*`）与动态规则选择器按设计不进入常驻工作区条。
- 边界四：控制中心 / 通知中心不做悬停切换（验收范围外）。

## 15. 关键工程取值一览

| 取值 | 值 | 所在 |
|---|---|---|
| 事件循环滴答 / 重同步去抖 | 200ms | `src/daemon/mod.rs` |
| 工作区规则合成 id 基准 | -100000 起递减 | `src/daemon/mod.rs` |
| 通知缓存上限 | 50 | `src/daemon/system.rs` |
| 窗口崩溃重启窗口 / 次数 / 延时 | 60s / 5 / 1s | `src/daemon/windows.rs` |
| 子进程退出轮询 | 300ms | `src/daemon/windows.rs` |
| 任务栏高度 | 30px | `src/bar/mod.rs` |
| UI 任务排空周期 | 40ms | `src/bar/mod.rs` |
| 请求超时 | 35s | `src/bar/ipc.rs` |
| 托盘图标尺寸 | 16px | `src/bar/modules/tray.rs` |
| 子菜单贴边外扩 | 5.0px | `src/bar/modules/tray.rs` |
| 悬停驻留（子菜单展开 / 图标切换） | 150ms | `src/bar/modules/popup.rs`、`tray.rs` |
| 合成器指针轮询周期 | 50ms | `src/bar/modules/popup.rs` |
| 图标垫色亮度阈值 / 采样上限 | 0.55 / 512 | `src/bar/modules/tray.rs` |
| 通知列表保留（窗口侧） | 20 | `src/bar/modules/window_info.rs` |
| 控制中心滑块节流 | 200ms | `src/bar/modules/center.rs` |
| 单帧上限 | 16 MiB | `src/shared/protocol.rs` |
