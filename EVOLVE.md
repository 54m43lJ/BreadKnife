# EVOLVE

## 通知弹窗（TODO 第 1 项）

- 需求：通知到达时，仅当前显示器在任务栏正下方居中弹出贴栏卡片；卡片样式与消失规则参考本机 dunst 配置，但标题改为居中，并在右上角提供关闭按钮。左键点击卡片触发默认动作后关闭该条（无动作时仅关闭），中键点击关闭该条。
- 实现思路：
  - 几何与样式沿用 dunst 配置的可见取值：宽 300px、单条最高 400px、圆角 15px、边框 1px、内边距 12px／水平 8px、图标 32px、图标与文字间距 16px、条间分隔线 1px；同屏上限 10 条，超出显示隐藏计数。
  - 覆盖 dunst 的两处默认：标题居中（dunst `alignment = left`），右上角增加关闭按钮（dunst 无此按钮）。
  - 消失规则按 dunst 的 urgency 超时落地：Low 30s，Normal / Critical 为 0（常驻至手动关闭）；客户端 `expire_timeout` 仍优先于宿主默认。
  - dunst 配置没有淡出动画项，因此「淡出规则」按超时／移除规则实现：到期或关闭即时移除卡片，不另设淡出动画；层表面动画交给合成器。
  - 新通知插入顶部、最新在最上，覆盖 dunst 的 `sort = yes`（不按紧急度重排）；右键关闭全部沿用 dunst `mouse_right_click = close_all`。
  - 交互按 TODO 覆盖 dunst 的鼠标绑定：左键触发默认动作并关闭（无动作仅关闭），中键关闭；动作按钮规则与通知中心共用。
  - `idle_threshold = 2m` 暂不纳入：Hyprland IPC 不提供输入空闲状态，宿主没有可靠的空闲源；当前只有 Low 存在 30s 超时，先按固定时长计时，后续接入空闲通知协议再补。
- 文档操作：

| 文档 | 章节 | 操作 | 内容 |
|------|------|------|------|
| `ABOUT.html` | 用户交互 · 界面交互（通知弹窗行） | 改 | 补充低优先级 30s 后消失、普通与紧急常驻至手动关闭，与 USER 契约对齐 |
| `USER.html` | 5.2.5 通知弹窗 | 改 | 补充标题居中、正文左对齐且中部省略、关闭按钮固定在右上角；堆叠注明不按紧急度重排；超时改为 Low 30s、Normal / Critical 常驻，到期即时移除且不另设淡出动画 |
| `CORE.html` | 4.13 bar/modules/info.rs | 改 | `POPUP_DEFAULT_TIMEOUT_MS` 改为 `POPUP_LOW_TIMEOUT_MS`；超时说明按紧急度分流，常驻条目不创建计时器 |
| `CORE.html` | 5.6.1 单条关闭 | 不动 | 计时器到期仍走 `NotificationClose { reason: Expired }` 路径，本次无需改动；动作与关闭实现与 TODO 通知能力项有重叠 |
| `USER.html` | 5.2.6 通知中心 | 不动 | 与 TODO 第二项重叠，本次不改通知中心的布局、聚合与消息排序 |
| `ABOUT.html` | 用户交互 · 界面交互（通知中心行） | 不动 | 同上，避免与 TODO 第二项冲突 |
| `bnote/PROVIDER.html` | 4.5 标准能力清单 | 不动 | 超时策略归宿主，bnote 能力契约不变；与 TODO 通知能力项重叠 |
| `bnote/USER.html` | 4.3 `Notice.expire_timeout_ms` | 不动 | 协议字段语义不变，默认超时仍由宿主解释 |
| `style.css` | 通知弹窗样例 `.mk-notif` | 不动 | 现有样例已符合 300px、15px 圆角、居中标题与右上关闭按钮，无新增视觉数值 |
| `TODO.md` | 第一项「收到 notification…」 | 改 | 追加 `[已解决]` 标记 |

## 通知中心（TODO 第 2 项）

- 需求：点击标题区域弹出消息中心，宽度占 40% 屏幕，高度随消息列表动态变化、最大为 100% 屏幕；消息按应用聚合，组按最后一条消息时间从新到旧排序（最后推送通知的应用在最上），点击聚合标题可收缩；每条消息标题至多 1 行、正文至多 2 行，超出裁剪并支持悬停查看全文；消息提供关闭按钮，左键触发动作并关闭、中键关闭；顶部提供一键清理全部按钮。
- 实现思路：
  - 面板顶边紧贴任务栏底边，水平中心对齐标题区域中心；宽 40% 屏宽，高度随列表增长，达到 100% 屏高上限后不再增高并在面板内部滚动。
  - 标题区域点击绑定 `InfoCenterWidget::toggle_center`：已打开则关闭，未打开则经 `PopupManager` 组 `info` 打开；打开/关闭同步 `NotificationWindow::set_center_open`，再次点击、点击弹窗外与 Escape 都走同一关闭路径。
  - 通知缓存按到达顺序新→旧保存；视图按 `app_name` 聚合，组按最后一条消息时间新→旧排序，组内新→旧；新通知到达时重新计算组顺序，最后推送的应用置顶。
  - 组标题点击展开/收缩，收缩态只保留聚合标题；组标题右侧关闭按钮发送 `NotificationCloseGroup` 清空本组；组内超过 3 条时只显示最新 3 条和「…」条目，展开时隐藏其它组。
  - 消息卡标题 1 行、正文 2 行，超出部分省略；完整标题/正文写入 tooltip，悬停卡片查看全文；关闭按钮与标题同行置右。
  - 左键点击消息卡发送 `NotificationActivate`（默认动作或唯一动作），触发后关闭该条，`resident` 除外；无动作时仅关闭。中键与关闭按钮发送 `NotificationClose { reason: Dismissed }`。
  - 顶部右上角清理按钮发送 `NotificationClear`，清空全部缓存与所有窗口的通知列表；组清理与单条关闭复用同一组请求，不新增协议消息。
  - 通知中心打开期间贴栏通知窗口隐藏但保留记录与计时，关闭后只恢复未过期的贴栏记录；视图开关与分组展开不修改通知缓存。
- 文档操作：

| 文档 | 章节 | 操作 | 内容 |
|------|------|------|------|
| `ABOUT.html` | 用户交互 · 界面交互（通知中心行） | 改 | 补充点击标题区域切换、再次点击/点击弹窗外/Escape 关闭、收缩态只保留聚合标题、悬停看全文与中键关闭 |
| `USER.html` | 5.2.6 通知中心 | 改 | 补充开关触发与关闭方式、收缩态、正文/标题裁剪后的悬停全文、中键与关闭按钮关闭该条 |
| `CORE.html` | 4.13 bar/modules/info.rs | 改 | 增加 `toggle_center` 签名；说明标题点击、popover 关闭信号与 `set_center_open` 的联动；补充收缩态与 tooltip 行为 |
| `CORE.html` | 5.8 通知中心开关 | 增 | 新增标题点击与 popover 关闭的调用栈 |
| `CORE.html` | 5.5 通知到达 | 不动 | 通知到达仍经 `upsert_center` 更新中心，本次只改打开/关闭与展示细节；与通知弹窗项共用同一到达路径 |
| `CORE.html` | 5.6 通知关闭、清理与动作 | 不动 | 单条关闭、组关闭、全部清理与动作回发沿用既有请求，本次未新增协议路径 |
| `USER.html` | 5.2.5 通知弹窗 | 不动 | 该项已在通知弹窗 TODO 中完成，本次不重复修改，避免与第 1 项冲突 |
| `bnote/PROVIDER.html` | 4.5 标准能力清单 | 不动 | 通知中心是宿主 UI，能力契约与协议字段不变；与通知能力项重叠 |
| `style.css` | 通知中心样例 `.mk-nc*` | 不动 | 现有样例已覆盖面板、分组、消息卡、更多条目与清理按钮，尺寸数值仍按注记与 CSS 为准 |
| `TODO.md` | 第二项「点击标题栏弹出消息中心…」 | 改 | 追加 `[已解决]` 标记 |

## 最小化托盘（TODO 第 3 项）

- 需求：加入类 macOS 的底部最小化托盘，鼠标进入屏幕底边时弹出；窗口以图标+标题横向排列，条目右侧带关闭键；左键点击恢复并聚焦窗口，中键点击关闭窗口；暂不实现右键菜单。
- 实现思路：
  - 以 Hyprland 特殊工作区 `special:minimized` 作为最小化集合：窗口移入即被收集，移出或关闭即移除；最小化动作由 Hyprland 配置或外部工具提供，bread 负责收集与恢复。
  - daemon 在启动、`movewindowv2` / `openwindow` / `closewindow` / `monitorremoved` / `configreloaded` 后查询 `clients -j` 重算集合；`windowtitlev2` 原地更新标题；列表顺序稳定，新条目追加右侧。
  - 支持 Hyprland `minimized` 事件：外部 taskbar 请求最小化时把窗口移入 `special:minimized`，请求恢复时移回显示器当前活动工作区并聚焦。
  - 每屏一条托盘，由窗口进程创建两个独立 layer-shell：2px 全宽底部触发条与动态宽度面板；均不 grab、不抢键盘焦点。指针进入触发条显示面板，离开面板/触发条 300ms 后收起。
  - 面板宽随条目增长，上限 88% 屏宽，超出横向滚动；条目宽 160px、高 48px，图标 32px、标题 1 行省略、关闭键右侧；面板 `surface` 底 `fg` 字、1px `gold` 边框，条目 Hover `purple` 底 `cream` 字。
  - 左键条目发送 `MinimizedRestore`，中键与关闭键发送 `MinimizedClose`；右键不绑定。恢复目标为窗口所属显示器的当前活动工作区，关闭使用 `closewindow`。
  - 数据面新增 `MinimizedWindow`（address / class / title / monitor），经 `StateDelta.minimized` 广播全量列表，各窗口按显示器过滤；新增两个请求类型，不新增推送类型。
  - 最小化动作不在 bread 内发起；BUILD 给出 Hyprland 键位示例，便于用户把窗口移入 `special:minimized`。
- 文档操作：

| 文档 | 章节 | 操作 | 内容 |
|------|------|------|------|
| `ABOUT.html` | 用户交互 · 界面交互（最小化托盘行） | 增 | 新增底部托盘的用户可见行为：悬停弹出、图标+标题、关闭键、左键恢复、中键关闭、无右键菜单 |
| `ABOUT.html` | 用户交互 · 界面交互（注记） | 增 | 说明 `special:minimized` 集合、每屏一条、恢复目标与最小化动作边界，并链接 BUILD |
| `USER.html` | 1. 术语 | 增 | 新增最小化工作区、最小化窗口、最小化托盘三个术语 |
| `USER.html` | 5.2.7 最小化托盘 | 增 | 新增托盘原型与完整视觉/交互契约：显隐、尺寸、颜色、排序、多显示器、恢复与关闭 |
| `style.css` | 最小化托盘样例 `.mk-dock*` | 增 | 新增托盘面板、条目、图标、标题与关闭键的原型式样式 |
| `CORE.html` | 1. 文件树 | 改 | 新增 `bar/modules/dock.rs` 与 `hyprland.rs` 的最小化集合职责 |
| `CORE.html` | 4.2 daemon/mod.rs | 改 | `Daemon` 增加 `minimized` 列表与 `MINIMIZED_WORKSPACE`；补充重同步、恢复、关闭、推送方法签名 |
| `CORE.html` | 4.3 daemon/hyprland.rs | 改 | `ClientInfo` 补 class / title / monitor，`WorkspaceRef` 补 name；新增 `move_to_workspace` / `focus_window` / `close_window` / `active_workspace_name` 与最小化相关事件 |
| `CORE.html` | 4.14 bar/modules/dock.rs | 增 | 新增 `DockWidget` 签名、两个 layer-shell 的触发/面板结构、显隐计时与常量 |
| `CORE.html` | 4.18 shared/protocol.rs | 改 | 新增 `MinimizedRestore` / `MinimizedClose` 请求与请求/推送语义说明 |
| `CORE.html` | 4.19 shared/model.rs | 改 | 新增 `MinimizedWindow`；`WindowSnapshot` / `StateDelta` 增加 `minimized` 列表 |
| `CORE.html` | 5.1 启动 | 改 | 窗口装配加入 `DockWidget`，daemon 启动重同步加入 `resync_minimized` |
| `CORE.html` | 5.9 最小化托盘 | 增 | 新增集合重同步、事件、恢复、关闭与 UI 显隐调用栈 |
| `PROVIDER.html` | 1. 术语、4.1 Hyprland IPC | 改 | 新增最小化工作区术语；登记 `movewindowv2` / `openwindow` / `closewindow` / `windowtitlev2` / `minimized` 事件、客户端查询字段与三个窗口 dispatcher |
| `BUILD.html` | 3.1 Linux、4. 补充说明 | 改 | 新增 Hyprland 最小化键位示例与托盘集成说明；补充「底部托盘不出现」的排查项 |
| `USER.html` | 5.2.5 通知弹窗、5.2.6 通知中心 | 不动 | 前两项已完成，最小化托盘不改通知的几何、生命周期与交互 |
| `bnote/PROVIDER.html` | 4.5 标准能力清单 | 不动 | 托盘是窗口管理 UI，不改变通知协议与服务能力；与通知能力项重叠 |
| `TODO.md` | 第三项「加入一个类macOS的底部托盘…」 | 改 | 追加 `[已解决]` 标记 |

## 当前显示器判定（TODO 第 4 项）

- 需求：确定通知弹窗如何获取当前显示器；比较 Hyprland `.socket.sock` 原始 IPC 查询与 `hyprctl` 命令两条路径，选定 daemon 的实现并写入契约。
- 实现思路：
  - 在 Hyprland 0.57.1 会话实测：`.socket.sock` 的 `j/monitors` 与 `j/activeworkspace` 返回与 `hyprctl -j` 完全一致的 JSON；本机单显示器环境均正确。
  - 性能实测（中位数）：原始 IPC `j/monitors` 约 0.03ms、`j/activeworkspace` 约 0.01ms；`hyprctl -j monitors` 约 1.97ms、`hyprctl -j activeworkspace` 约 1.91ms。原始 IPC 更快，但 `hyprctl` 的 2ms 对通知级频率可忽略。
  - 选定 `hyprctl -j monitors` 的 `focused` 字段：复用 daemon 既有的 `hyprctl -j` 查询路径，不引入第二个 Hyprland socket 客户端，也避免一次性连接未关闭导致 Hyprland 最长 5s 阻塞的风险；socket2 继续保持只读。
  - `current_monitor()` 每次通知到达时查询，保持“当前显示器”精确；查询失败或无 focused 显示器时回退首个显示器，无显示器时返回 `None` 并跳过贴栏弹窗，通知中心仍正常缓存。
  - 不缓存 `focusedmonv2` 事件作为弹窗目标，避免跨线程事件顺序导致焦点切换与通知到达竞争；事件流只负责既有状态推送。
  - `MonitorInfo` 增加 `focused` 字段；PROVIDER 的查询表登记 `focused` 的消费。单显示器环境无法实测多屏焦点切换，多屏语义以 Hyprland `focused` 字段为准。
- 文档操作：

| 文档 | 章节 | 操作 | 内容 |
|------|------|------|------|
| `CORE.html` | 4.3 daemon/hyprland.rs | 改 | `MonitorInfo` 增加 `focused`；`current_monitor` 注明取 focused、查询失败回退首个 |
| `CORE.html` | 5.5 通知到达 | 改 | 调用栈补 `hyprctl -j monitors` 查询与回退；注明无显示器时跳过贴栏弹窗、中心缓存不受影响 |
| `USER.html` | 5.2.5 通知弹窗 | 改 | 明确当前显示器取 Hyprland 焦点显示器，查询失败回退首个显示器 |
| `PROVIDER.html` | 4.1 Hyprland IPC（查询表与注记） | 改 | 登记 `monitors -j` 的 `focused` 字段；说明不启用 `.socket.sock` 查询，socket2 保持只读 |
| `ABOUT.html` | 用户交互 · 界面交互（通知弹窗行） | 不动 | 用户可见行为仍是「仅当前显示器弹出」，本次只落实现判定与失败回退 |
| `BUILD.html` | 运行前提 / 排查 | 不动 | 无新增依赖或用户配置；`hyprctl` 已是既有查询路径 |
| `bnote/PROVIDER.html` | 4.5 标准能力清单 | 不动 | 当前显示器判定属于宿主 UI，不改变通知协议与能力清单 |
| `TODO.md` | 第四项「通知弹窗当前显示器判定…」 | 改 | 追加 `[已解决]` 标记 |

## 九色色板迁移（TODO 第 5 项）

- 需求：按权威九色色板迁移实现分支的 `src/assets/bread.css` 与 `src/`，清除旧 token 名、托盘动态垫高、`success` 等遗留，完成整体色板迁移；文档侧的色板定义已经定稿，本次让代码与契约对齐。
- 实现思路：
  - 以 `bg / surface / fg / muted / purple / purple-light / gold / cream / red` 九色替换旧 token：`bg_primary→bg`；`bg_secondary` / `bg_hover→surface`；`bg_active→fg`（滑轨）；`fg_primary→fg`；`fg_secondary→fg`（文字）或 `muted`（图标）；`fg_muted→muted`（通知应用名改 `gold`）；`accent` / `accent_hover→purple` / `purple-light`；`warning→gold`；`danger→red`；`border` / `border_strong→gold`；`success` 删除且不设替代。
  - 工作区条紧急态改为两色互换：基底 `purple→fg`、叠加点 `fg→purple`；Rust 的 `on-warning` 类改名为 `on-urgent`，与 USER §5.2.2 对齐。
  - 托盘取消按图标亮度动态垫高：删除 `apply_pad_class` / `icon_is_dark` / `texture_luminance` / `luminance_of_samples` 与 `tray-pad-*` 样式；图标按钮直接以 `fg` 为底色渲染。托盘菜单随色板改为 `fg` 底、`bg` 字，Hover `purple` 底 `cream` 字；注意态用 `gold` 微高亮。
  - 控制中心入口改透明底；滑轨取 `fg`、滑块取 `purple`、Hover/已填充取 `purple-light`；日历选中项取 `purple` 底 `cream` 字。
  - 代码分支 `refactor/palette-migration`，提交 `f2f06a2`；`cargo check` 通过，`cargo test` 21 项全部通过。
  - 文档契约不变：USER §5.1 已定义九色，CORE 的依赖与 SBOM 不因删除亮度采样而变化。
- 文档操作：

| 文档 | 章节 | 操作 | 内容 |
|------|------|------|------|
| `src/assets/bread.css` | 全文件 | 改 | 九色 token 替换旧色板；工作区紧急态两色互换；托盘/菜单/控制中心/滑块按契约取色；删除 `tray-pad-*`、`success`、`warning`、`danger` 等旧项 |
| `src/bar/modules/tray.rs` | 托盘渲染 | 改 | 删除亮度采样与动态垫高调用及全部辅助函数；图标直出 |
| `src/bar/modules/workspace.rs` | 工作区渲染 | 改 | 紧急叠加点 CSS 类由 `on-warning` 改为 `on-urgent` |
| `USER.html` | 5.1 样式基础变量 | 不动 | 九色色板已是权威定义；本次是代码对齐，不改契约 |
| `CORE.html` | SBOM / 内部结构 | 不动 | 无新增依赖；删除亮度采样不改变对外接口与构建方式 |
| `style.css` | 文档原型 | 不动 | 原型已使用九色，不需随代码迁移修改 |
| `BUILD.html` | 构建与排查 | 不动 | 构建命令与运行前提不变 |
| `TODO.md` | 第五项「实现分支重构任务…」 | 改 | 追加 `[已解决]` 标记 |

## 通知能力实现（TODO 第 6 项）

- 需求：按 `bnote/PROVIDER §4.5` 的目标实现通知能力：动作按钮与 `action-icons`、正文 markup／超链接／图片、提示音、内存持久化与 `transient`／`resident`；`bnote` 需暴露对应 hint；`x-vendor-*` 不实现。
- 实现思路：
  - `bnote` 侧补齐 `Notice` 的 `action_icons` / `transient` / `resident` / `sound_file` / `sound_name` / `suppress_sound` 字段，`hints.rs` 解析同名 hint：布尔缺失或类型不符→`false`，文本缺失、类型不符或空串→`None`；`build_notice` 原样投影。
  - `bnote` 实现分支 `feat/notification-hints`，提交 `c42e22b`；`cargo test --manifest-path bnote/Cargo.toml` 通过（单元 28 项、协议集成 8 项）。
  - 提示音后端定为 `canberra-gtk-play`：`suppress_sound` 优先跳过；`sound_file` 用 `-f`，`sound_name` 用 `-i`；命令异步 spawn、不阻塞通知展示，失败只记日志。启动时检测 PATH，后端缺席则宿主不声明 `sound` 能力。
  - 正文按 `body-markup` 解析规范允许的标记，不支持标记降级纯文本；`body-hyperlinks` 仅 `http` / `https` / `mailto` 可点击，经 `xdg-open`，其余协议按纯文本；`body-images` 仅本地 `file://` 与图标主题名，不访问网络，按比例缩放，失败降级替代文本。
  - 动作为既有契约：默认文字，`action-icons` 时以 key 为图标名、label 为 tooltip，未命中回退文字；左键触发 default / 唯一动作后关闭，`resident` 保留。
  - 持久化沿用既有契约：条目内存保留至 daemon / 会话结束；`transient` 绕过，超时即删除；`resident` 动作后保留。
  - `icon-multi` 本期不实现：`NoticeIcon` 只承载单帧，多帧需要协议参数、模型与渲染链路扩展；宿主继续声明 `icon-static`。`x-vendor-*` 扩展不声明。
  - 宿主通知 UI 的渲染与生命周期按 USER §5.2.5／§5.2.6 与 CORE §4.13 的既有契约执行；本项闭合 bnote 暴露面与提示音后端等剩余契约缺口。
- 文档操作：

| 文档 | 章节 | 操作 | 内容 |
|------|------|------|------|
| `bnote/src/types.rs` | `Notice` | 改 | 增加 `action_icons` / `transient` / `resident` / `sound_file` / `sound_name` / `suppress_sound` 字段 |
| `bnote/src/hints.rs` | hints 解析 | 改 | 解析布尔与文本 hint，补齐缺省回退与能力 hint 测试 |
| `bnote/src/server.rs` | `build_notice` | 改 | 将 hints 派生字段投影到 `Notice`，同步测试 |
| `bnote/PROVIDER.html` | 4.5 标准能力清单 | 改 | `sound` 登记 canberra-gtk-play 后端；明确 `icon-multi` 本期不实现；链接正文渲染契约 |
| `USER.html` | 5.2.5 通知弹窗 | 改 | 补充 body-markup／hyperlinks／images 渲染与降级；提示音后端与能力缺席行为 |
| `CORE.html` | 4.6 daemon/notify.rs、5.5 通知到达 | 改 | 新增 `SOUND_BACKEND` / `sound_available` / `play_sound` 签名与到达时播放调用 |
| `PROVIDER.html` | 4.3 外部调节工具 | 改 | 增加 `canberra-gtk-play` 与 `xdg-open` 的消费项与缺席行为 |
| `BUILD.html` | 1. 开发者准备 | 改 | 可选系统工具补充提示音与链接处理后端 |
| `bnote/USER.html` | 4.3 数据模型与转换 | 不动 | `Notice` 字段与 hint 语义已在此定义，本次只补代码与外围契约 |
| `bnote/CORE.html` | 4.4 hints.rs | 不动 | 解析职责与字段归属已定义，本次代码按此实现 |
| `USER.html` | 5.2.6 通知中心 | 不动 | 动作/消息交互与弹窗共用同一契约，通知能力项不重复修改 |
| `TODO.md` | 第六项「通知能力实现…」 | 改 | 追加 `[已解决]` 标记 |

## 资源占用与能耗测试（TODO 第 7 项）

- 需求：搭建资源占用与能耗测试手段，用于优化峰值 CPU 与对应能耗、内存占用，并把常态 CPU 压到接近 0；运行时应保持事件驱动，只允许少量低频轮询。
- 实现思路：
  - 新增 `scripts/resource-smoke.py`：从 `/proc/PID/stat`、`status`、`smaps_rollup` 采样 daemon 与 `bread __window` 进程的平均/峰值 CPU、RSS/PSS、上下文切换率；按 0.25s 间隔采样并跟踪热插拔期间新出现的窗口进程。
  - 支持 `--during` 注入场景命令（通知突发、工作区切换、托盘交互等），`--json` 输出用于存档与回归比较；`--max-avg-cpu` / `--max-peak-cpu` / `--max-rss-mb` / `--max-energy-j` 提供阈值退出码。
  - 能耗优先读取 RAPL `intel-rapl:*` 包级 `energy_uj`；不可读时输出 N/A 并以电池 `power_now` 作为笔记本替代参考；同时记录 `/proc/pressure/cpu` 供峰值/能耗联合判断。
  - 固定流程：先测 idle 30s 基线，再逐场景注入，同机同会话重复比较相对变化；不以跨机器绝对值作结论。
  - 代码分支 `test/resource-smoke`，提交 `e481fe1`；脚本已在当前会话冒烟运行（3s idle 采样通过，RAPL 因权限为 N/A）。
  - 优化目标本身保持为后续任务：本项只建立可重复的测量与回归手段，不预设具体阈值；阈值由实际基线确定后写入脚本参数或 CI。
- 文档操作：

| 文档 | 章节 | 操作 | 内容 |
|------|------|------|------|
| `scripts/resource-smoke.py` | 新增脚本 | 增 | 资源/能耗采样：CPU、RSS/PSS、上下文切换、RAPL/电池、CPU pressure、场景注入与阈值 |
| `BUILD.html` | 1. 开发者准备 | 改 | 新增 Python 3 依赖项，说明冒烟与资源采样脚本 |
| `BUILD.html` | 2. 开发命令、2.1 资源与能耗采样 | 增 | 增加采样命令、指标来源/判读表、idle→场景→重复的固定流程与阈值用法 |
| `CORE.html` | 1. 文件树 | 改 | 登记 `scripts/resource-smoke.py` 并注明其开发期工具定位 |
| `USER.html` | 全部 | 不动 | 资源测试不改变用户可见行为与接口 |
| `PROVIDER.html` | 全部 | 不动 | RAPL/电池/压力仅由开发期脚本消费，不进入 bread 运行时依赖 |
| `ABOUT.html` | GOAL / 用户交互 | 不动 | 测试手段不改变产品能力描述 |
| `bnote/*` | — | 不动 | 资源测试不触及通知协议库 |
| `TODO.md` | 第七项「搭建一些资源占用和能耗相关的测试手段…」 | 改 | 追加 `[已解决]` 标记 |
