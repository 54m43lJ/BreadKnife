# breadpan 与 astal-tray 能力边界对照（工作稿）

> 定位：集成决策用——按能力域列出 astal-tray（现状）与 breadpan（目标）的边界差别及应对方案。
> API 级替换映射与工作量估算见 [MIGRATION.md](MIGRATION.md)；动作线条见 [INTEGRATION.md](INTEGRATION.md)。

## 总览

- 一句话边界：**astal-tray 交付"可直接渲染的 GTK 件"；breadpan 交付"协议栈 + 数据流"**——
  astal 把菜单渲染、图标转换（GIcon）交给 GTK 基础设施；breadpan 交付快照 / 事件 / 菜单快照 / 可绘制对象，
  UI 组装与菜单渲染归应用。
- 差别集中在三处：① 组装实例模型（每屏多实例 → 进程级单实例）；
  ② 菜单（GTK 模型 → 快照自绘）；③ 事件模型（属性推送 → 事实事件 + 现场重读）。
- 反向差异按类型分三种：**规格补全**（中键、四向滚轮、Tooltip）、**诚实状态**（存活置灰、双注册自愈、换主事件）、
  **工程收益**（纯 Rust 依赖、显式停机与终局语义）——按文档契约直接纳入，不是迁移债。

## 逐域对照

| 能力域 | astal-tray（现状） | breadpan（目标） | 差别性质 | 应对方案 |
|---|---|---|---|---|
| 组装模型 | 每显示器一个 `Tray` 实例 | `Tray::start` 进程级一次（Watcher 名独占、单事件通道） | 结构性：实例 1↔N | 托盘服务上提为进程级；事件按屏扇出；启动失败（`NameTaken` 等）降级空托盘 + 日志 |
| 运行时与构建 | 系统级 libastal(-tray) / appmenu-glib-translator 等构建依赖 | 纯 Rust 依赖（zbus / tokio / thiserror；GTK 整合为可选特性）；公开面全同步 | 构建面收缩 | 删系统构建依赖与 CI 步骤；应用无需装配 async 运行时 |
| Item 发现 | `items()` 启动全量 + `item_added/removed` 信号 | 首事件 `Snapshot`（原子快照）+ `Added/Removed/Changed` 事实事件 | 推模型 → 拉模型 | 事件泵驱动图标增删改；取值一律重读 `item(id)` |
| 图标 | `gicon()` + `connect_gicon_notify`（GIcon） | `IconSource`（主题名 / 文件路径 / 位图）→ `icon_paintable()` 渲染直出 `gdk::Paintable` | 直出替代推送 | `Image::from_paintable` 上屏；变更由 `Changed` 重读驱动；文件路径图标（Ayatana 生态）库侧已支持 |
| 左键 | `activate(x, y)` | `left_click(_at)`；`menu_only` 项自动转菜单流程 | 返回值分流 | `Outcome::Menu` → 弹菜单；`Dispatched` → 结束 |
| 中键 | `secondary_activate(x, y)`（现挂在右键无菜单回退上） | `middle_click(_at)` → SecondaryActivate | astal 侧无干净对应 | 中键独立接线；最小等价档位下可后补（应用文档 §5.3 已列） |
| 右键菜单 | `menu_model() + about_to_show() + action_group()` → `PopoverMenu::from_model` | `right_click(_at)` → `Outcome::Menu(MenuSnapshot)`（AboutToShow 内置）；`menu_expand` 子层懒加载、`menu_activate` 下发、`MenuChanged` 刷新 | 交付面：GTK 模型 → 快照；渲染与键盘导航归应用 | 自绘 `gtk::Popover`（推荐路线，MIGRATION §6.1）；勾选态忠实透传、无乐观更新、重读刷新 |
| ContextMenu 回退 | 现实现未出现 | `Outcome::Dispatched`：Item 自绘菜单，宿主不渲染 | breadpan 显式化 | 无需渲染分支，按回执结束 |
| 滚轮 | 未使用 | `scroll_up/down/left/right(_by)`（垂直 + 水平，步数聚合） | 规格补全 | 滚轮手势接线（Shift+滚轮转水平） |
| Tooltip | 未使用 | ToolTip 数据交付 | 规格补全 | 悬停直接显示 |
| 健康状态 | 未使用 | 心跳探测 → `alive`；`is_alive(id)` | 诚实状态 | `alive=false` 置灰（不移除；移除只由 `Removed` 驱动） |
| 换主 / 双注册 | 无对应事件 | `OwnerChanged`（同名换主）、`NameChanged`（唯一名升级知名名并入） | 事件面扩展 | 按事件重键重读；不再出两张图标 |
| 线程模型与阻塞面 | GObject 信号（主线程回调） | 公开面全同步：阻塞**调用线程**；库内无跨 Item 阻塞（每调用独立异步任务；无跨 await 锁；事件满则丢弃计数） | 阻塞范围 = 调用线程 × 该调用链 | 立即返回类：中键 / 滚轮 / 右键 fallback / `menu_activate`；同步链类：左键 ≈2s（回退菜单链 ≤10s）、菜单读取 ≈4–6s、`item()` ≈2–4s、`items()` ≈2–4s；单次上限 `sync_guard_ms`（30s 兜底）。应用若从 GTK 主线程同步调用，冻结为**整条任务栏**——按"整条任务栏冻结不可接受"标准，应把交互与重读放到工作线程、结果回投主上下文（详录见 MIGRATION §6.3） |
| 停机与终局 | 无显式停机入口；终局语义未定义 | `shutdown()` / 句柄 drop 显式释放（Watcher 名、内部线程）；`Fatal` / `Displaced` 终局不自动重连 | 语义显式化 | 退出路径显式停机；重连与否待拍板（+0.5 人日候选） |

## 应用侧落点

- 必修面（组装模型、Item 生命周期、图标、交互）是等价迁移的工作量主体，估算见 MIGRATION.md §4。
- 最大单项为菜单（自绘）：键盘导航、助记符、勾选态呈现需自研——路线选择见 MIGRATION.md §6.1。
- 规格补全面（中键 / 滚轮 / Tooltip / 存活 / 换主）按应用 ARCHITECTURE §5.3 契约纳入，可随迁移一次做完或分阶段。

## 依赖面核对

- 本分支的正式文档（应用 README / ARCHITECTURE / CODE 与 breadpan 文档）已无 astal 提及，依赖叙事均为 breadpan；
  残留的 astal 表述只在本工作稿与 [MIGRATION.md](MIGRATION.md)（迁移语境）。
- 仍在应用代码侧的 astal 面：`Cargo.toml` 的 astal-tray 依赖与 `src/bar/control_center.rs`（dev/breadpan-2）、
  main 的 `README.md`、CI 的 libastal / appmenu-glib-translator 构建步骤——按 MIGRATION.md P0 / P6 处理。
