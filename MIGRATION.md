# astal-tray → breadpan 托盘迁移评估

> 定位：迁移工作量评估记录（工作稿），不属于四文档体系；托盘库契约以
> [breadpan/ARCHITECTURE.html](breadpan/ARCHITECTURE.html) 为准，应用侧托盘规格见
> [ARCHITECTURE.html §7.3](ARCHITECTURE.html) 与 [CODE.html §3.4](CODE.html)。

## 1. 结论摘要

- 迁移面集中：应用侧只有 `src/bar/control_center.rs`（148 行）直接依赖 astal-tray，
  其余代码（daemon / hyprland / workspace / title）与托盘无关。
- 结构性差距一条：astal 的 `Tray` 当前按显示器各建一个实例；breadpan 的 `Tray::start`
  是**进程级单实例**（Watcher 总线名独占、单 Receiver、无追加订阅），需要把托盘组装
  上移为进程级服务，再向每个显示器的 UI 扇出事件。
- 最大工作量在右键菜单：astal 直接交付 `GMenuModel + GActionGroup`，
  `PopoverMenu::from_model` 一行渲染；breadpan 交付 `MenuSnapshot`，
  **GTK 弹出菜单由消费方组装**（含子层 `menu_expand` 懒加载与 `menu_activate` 下发）。
- 估算：约 **5–6.5 人日**（单人全脱产，含真机联调）；若只追求与现有 astal UI 等价、
  暂不补中键/滚轮，约 **4–5 人日**。
- 依赖收益：移除 `astal-tray` / `astal-tray-sys` 与系统级 libastal(-tray) 构建依赖；
  breadpan 仅 Rust 依赖（zbus / tokio / thiserror / gtk4），公开面全同步、无需消费方
  装配 async 运行时。

## 2. 现状（main 已并入 dev/breadpan 后）

- 本分支现同时包含：应用代码（`src/`、根 `Cargo.toml`，来自 main）与 breadpan 库
  （`breadpan/`，本分支开发）；合并时已排除 `.github/`（CI 仍在 main）。
- 应用侧托盘实现只有 `src/bar/control_center.rs`：托盘图标盒 + 时钟，每个显示器
  一个 `ControlCenter`，各自 `Tray::default()`。
- 库侧 breadpan 已完整实现 SNI + DBusMenu 协议栈（登记中心内置、零缓存现场读取、
  逐层菜单懒加载、事件流、GTK 渲染直出），`cargo check` 通过，带全量 mock 测试
  （`dbus-run-session -- cargo test`）。
- 应用侧文档（ARCHITECTURE §4.3/§7.3、CODE §3.4、BUILD 依赖表）已按 breadpan 契约
  写好，本次迁移是让代码追平文档；`README.md`（来自 main）仍在宣传 Astal，需清理。

## 3. 现状 astal 使用面清单

| astal API | 位置 | 用途 |
|---|---|---|
| `Tray::default()` | `control_center.rs:23` | 每显示器实例化 |
| `tray.items()` | `:30-33` | 启动时全量建图标 |
| `connect_item_added` / `connect_item_removed` | `:42-47` / `:54-58` | 增删图标 |
| `TrayItem::gicon()` + `connect_gicon_notify` | `:91` / `:98-100` | 图标渲染与变更 |
| `TrayItem::activate(x, y)` | `:108-111` | 左键 |
| `TrayItem::secondary_activate(x, y)` | `:134` | 右键无菜单时的回退（当前代码把它挂在右键） |
| `TrayItem::menu_model()` + `about_to_show()` + `action_group()` + `PopoverMenu::from_model` | `:122-132` | 右键菜单 |
| `Cargo.toml:11` | `astal-tray = "0.1.0"` | 唯一 astal 依赖 |

## 4. 目标结构与工作分解

建议的最小结构（不为迁移做无关的模块重组，文档规划的 `src/modules/*` 属于后续任务）：

```
src/
├── tray.rs                 # 新增：进程级托盘服务（组装 + 事件泵 + 多显示器扇出 + 停机）
├── bar/
│   ├── mod.rs              # 小改：把托盘服务传给每屏 ControlCenter
│   ├── control_center.rs   # 重写：订阅事件、图标增删改、手势接线
│   ├── tray_menu.rs        # 新增：MenuSnapshot → GTK 弹出菜单
│   └── style.css           # 小改：菜单行/分隔符/toggle 样式
├── daemon.rs               # 小改：退出路径触发 tray 停机
└── main.rs                 # 小改：启动组装时序（如需）
```

| 阶段 | 内容 | 估时（人日) |
|---|---|---|
| P0 依赖切换 | `Cargo.toml` 换为 `breadpan = { path = "breadpan" }`、锁文件更新、`cargo check` 通过 | 0.25–0.5 |
| P1 托盘服务 | 进程级 `Tray::start` 一次组装；`glib::spawn_future_local` 事件泵；多显示器订阅/扇出；daemon 退出时 `shutdown()`；start 失败的降级（空托盘 + 日志） | 0.75–1.25 |
| P2 Item 生命周期 | `Snapshot/Added/Removed/Changed` → 每屏图标增删改；`gtk::icon_paintable(&item.icon)` 渲染直出；`Changed` 重读 `item(id)`（替代 astal 的 gicon 推送） | 0.5–0.75 |
| P3 交互接线 | 左/中/右键与四向滚轮手势 → `left_click_at` / `middle_click_at` / `right_click_at` / `scroll_*`；`InteractionOutcome::Menu` 分流到菜单弹窗 | 0.5–0.75 |
| P4 菜单弹窗 | `MenuSnapshot` → GTK 弹出：子层 `menu_expand` 懒加载、toggle/图标/禁用/可见/分隔行、点击 `menu_activate`、弹窗打开期间 `MenuChanged` 重取刷新 | 1.5–2.5 |
| P5 真机联调 | Hyprland + 真实 Item（QQ 音乐、clash-verge）对照旧版验收；多屏插拔；退出清理；Fatal/Displaced 降级 | 0.75–1.25 |
| P6 文档清理 | `README.md` 去 Astal/libastal 说明；样式与死代码；CI（另行合回 main 时删除 astal / appmenu-glib-translator 构建步骤） | 0.25–0.5 |
| **合计** | | **4.5–7.5（较可能 5–6.5）** |

代码量估算：新增/重写约 600–800 行（`tray.rs` ≈120–160，`tray_menu.rs` ≈250–350，
`control_center.rs` 重写后 ≈150–190，接线与样式 ≈60–90），净增约 450–650 行。

## 5. API 对照（替换映射）

| 现状（astal） | breadpan 目标 | 说明 |
|---|---|---|
| `Tray::default()` × 每显示器 | `Tray::start(TrayConfig)` × 每进程一次 | 单实例约束；失败返回 `NameTaken` 等错误 |
| `tray.items()` | 快照事件 `TrayEvent::Snapshot { items }`（首事件即快照） | 无需再全量现场读；`handle.items()` 保留作全量刷新 |
| `tray.item(id)` | `handle.item(id) -> Option<TrayItem>` | 直接对应 |
| `connect_item_added/removed` | `TrayEvent::Added/Removed` | 事件驱动；唯一 Receiver 需应用扇出 |
| `gicon()` + `connect_gicon_notify` | `icon_paintable(&item.icon) -> gdk::Paintable` + `Changed` 重读 | 推模型 → 拉模型；`Image::from_paintable` |
| `activate(x,y)` | `left_click_at(id,x,y)` | menu_only 项自动转菜单流程，返回 `Outcome::Menu` 需处理 |
| `secondary_activate(x,y)` | `middle_click_at(id,x,y)` | 当前 UI 未接中键，按文档 §7.3 建议补 |
| `menu_model()+about_to_show()+action_group()` | `right_click_at(id,x,y) -> Outcome::Menu(MenuSnapshot)` | AboutToShow 内置；`Dispatched` 即 ContextMenu 回退，宿主不渲染 |
| `PopoverMenu::from_model` | 自绘弹出 + `menu_activate(id, entry_id)` + `menu_expand(id, entry_id)` | 迁移最大单项；toggle/radio 状态为忠实透传，无乐观更新，靠重读刷新 |
| （无） | `scroll_up/down/left/right(_by)` | 文档 §7.3 已纳入规格 |
| （无） | `is_alive(id)`、`tooltip`、`overlay/attention_icon` | 可选增强；`alive=false` 置灰属库侧"诚实状态呈现"设计 |

## 6. 风险与开放问题

1. **菜单自绘是最大不确定项**：键盘导航、助记符、勾选态呈现都需自研；astal 版
   `PopoverMenu::from_model` 的 UX 不能免费获得。两条路线：
   A. 自绘 `gtk::Popover`（推荐：子层懒加载可控，预估已含）；
   B. `MenuSnapshot → GMenuModel + SimpleActionGroup + PopoverMenu`（复用 GTK 渲染与
   键盘导航，但子菜单懒加载难以按需触发，工作量相当、懒加载体验更差）。
2. **图标文件名回归**：breadpan 对"`IconName` 为文件路径"（Ayatana 生态，如
   clash-verge）回退 `image-missing`（见 IMPLEMENTATION §6）；需确认 astal 旧行为，
   若不接受则属 breadpan 渲染层增强（约 0.5 人日，不在此应用侧估算内）。
3. **同步接口阻塞 GTK 主线程**：`items()/_click()/menu()` 经内部同步桥等待 DBus，
   单次受 `timeout_ms`（默认 2s）约束；病态 Item 可能短暂卡 UI。启动期可接受，
   后续可把交互卸载到工作线程（`TrayHandle` 是 Send+Sync）。
4. **单 Receiver、单 Watcher 名**：多显示器需要应用内扇出；同会话不可与其他 SNI
   host 并存（旧 astal 版本进程未退出时会 `NameTaken`），联调时注意。
5. **Fatal/Displaced 无自动重连**：breadpan 语义为终局（不重试不夺回）。需决策：
   仅记录降级，还是监听后重启 `Tray::start`（+0.5 人日候选）。
6. **`MenuChanged` 刷新语义**：弹窗打开期间需重取菜单（根层或已展开路径）并重建；
   当前 astal 版无此逻辑。
7. **双注册双 id**（IMPLEMENTATION §6）：个别应用会出两张图标，属库已知限制，
   迁移后需真机观察是否影响使用。
8. demo-tray 目前只有文档、无实现（示例仅 `breadpan/examples/*`）；不影响本迁移，
   但"验收基准"操作矩阵可借鉴其 ARCHITECTURE §7。

## 7. 验证计划

- 构建：`cargo check/build`（不再需要 libastal 系统依赖）；`cd breadpan &&
  dbus-run-session -- cargo test`（库全量测试）。
- 真机矩阵（Hyprland + DBus，对照 demo-tray/ARCHITECTURE §7 验收表与旧 astal 版行为）：
  - 启动：快照一次到位，多显示器图标一致；
  - 生命周期：启停应用 → `Added/Removed`；状态/图标变更 → `Changed` 重读生效；
  - 交互：左键（含 menu_only 项）、中键、右键（DBusMenu 菜单与 ContextMenu 两路）、
    四向滚轮；
  - 菜单：子层懒加载展开、toggle 点击、菜单打开期间 `MenuChanged` 刷新；
  - 退出：socket `exit` 后 Watcher 名释放、无残留图标/占用。
- 回归对照：与替换前的 astal 版二进制并行观察（同一天、同组应用）。

## 8. 估算汇总

| 视角 | 估算 |
|---|---|
| 最小等价迁移（图标+左右键+菜单，不加中键/滚轮） | 约 4–5 人日 |
| 文档契约全量（含中键、四向滚轮、状态增强） | 约 5–6.5 人日（较可能值），上下限 4.5–7.5 |
| 变化面 | 1 个文件重写、2 个新文件、4 处小接线；净增 450–650 行 |
| 库侧阻塞项 | 无；可选增强：文件路径图标（0.5 人日，breadpan 侧） |
