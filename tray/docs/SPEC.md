# Tray 库规格说明（SPEC）

**库名**: `tray`
**文档版本**: v0.2（PoC 实现随本版本落地）
**上游需求**: [PRD-1.0.md §3.3 系统托盘模块](../../PRD-1.0.md)
**状态**: 本文档是该库的唯一权威规格，PRD 及上层代码只允许引用本文档描述的接口，不得感知库的内部实现。

---

## 1. 定位与边界

### 1.1 本库是什么

`tray` 是一个 **无头（headless）的 Linux 系统托盘数据层库**，职责是：

- 完整实现 StatusNotifierItem（SNI）协议的客户端侧：发现、跟踪、查询托盘项；
- 内置 Fallback Watcher，在无外部 Watcher 的裸会话（如 Hyprland）中开箱即用；
- 完整实现 DBusMenu（`com.canonical.dbusmenu`）客户端：托盘项的右键菜单数据与交互；
- 以统一的事件流（回调原语 + 事件通道封装）向消费方投递托盘世界的全部变化。

### 1.2 本库不是什么（Non-goals）

| 不做的事 | 说明 |
| :--- | :--- |
| 不做任何 UI | 不依赖 GTK/GLib/Astal，不产生任何 Widget。渲染由消费方完成 |
| 不解析图标主题 | 只交付 `IconName` 字符串与原始位图（Pixmap），是否查主题、如何 fallback 由消费方决定 |
| 不做样式 | PRD 中的"高对比度边框/深浅色智能选择"是 UI 层职责，本库只提供原始像素与元数据 |
| 不做 XEmbed/X11 传统托盘 | 仅支持 SNI（Wayland 时代协议） |
| 不做菜单渲染/弹出定位 | 本库交付菜单数据模型与激活事件，弹窗由消费方实现 |
| 不管理配置文件 | 全部行为由构造参数显式传入 |

### 1.3 独立性硬约束

1. 本库**不得**依赖本仓库任何其他模块（workspace 内零反向依赖）；
2. 硬依赖白名单仅限：`zbus`（DBus 通信，含服务端能力）、`thiserror`（错误定义）、标准库；
3. 事件通道使用 `std::sync::mpsc`，不引入额外通道库；
4. 不得引入任何异步运行时（tokio 等）到公开 API 中——内部使用 zbus 自带的 async-io 执行器，外部无感；
5. 本库必须能在 `cargo build` 下独立编译、在 `cargo test` 下独立测试，不要求消费方工程存在。

---

## 2. 术语

| 术语 | 含义 |
| :--- | :--- |
| **Watcher** | 总线上的 `org.kde.StatusNotifierWatcher` 服务，负责登记托盘项与宿主 |
| **Host** | 托盘图标的展示方（即本库）。注册 `org.kde.StatusNotifierHost` 并向 Watcher 登记 |
| **Item** | 一个托盘项，即某个应用在总线上导出的 `org.kde.StatusNotifierItem` 对象 |
| **Menu** | Item 通过 `Menu` 属性指向的 `com.canonical.dbusmenu` 对象 |
| **会话总线** | DBus session bus，本库全部通信发生在此 |

---

## 3. 能力清单

| # | 能力 | 说明 |
| :--- | :--- | :--- |
| C1 | Watcher 跟踪 | 监视 Watcher 的名字所有者变化；Watcher 消失/出现时自动重连、触发 Item 重新发现 |
| C2 | Host 注册 | 以唯一名 `_org.kde.StatusNotifierHost.<pid>-<序号>` 注册，并调用 Watcher 的 `RegisterStatusNotifierHost` |
| C3 | Item 发现 | 接收 `StatusNotifierItemRegistered/Unregistered` 信号；并对已注册 Item 做掉线自愈（唯一名消失即移除） |
| C4 | Item 属性同步 | 拉取并跟踪 SNI 规范全部属性；监听 `PropertiesChanged`，合并后投递变更事件 |
| C5 | 图标数据 | 交付 `IconName`、`IconPixmap`（ARGB32 原始位）、Attention/Overlay 系列图标，二选一规则见 §5.3 |
| C6 | Tooltip | 交付 SNI `ToolTip` 结构（图标 + 标题 + 描述） |
| C7 | 交互调用 | `Activate(x,y)`、`SecondaryActivate(x,y)`、`Scroll(delta, orientation)` |
| C8 | DBusMenu 客户端 | 菜单布局拉取、增量同步、只读快照模型、`AboutToShow`、条目点击事件下发 |
| C9 | Fallback Watcher | 无外部 Watcher 时自动补位提供 Watcher 服务；外部 Watcher 出现时自动让位 |
| C10 | 事件投递 | 回调注册为底层原语；通道订阅为其上的默认封装（§6） |
| C11 | 测试替身 | `testing` 特性提供可编程的假 Item，供无托盘应用环境下的开发与 CI（§10） |

---

## 4. 运行模型

```
┌────────────────────────────┐
│        消费方（如 BreadKnife）        │
│  UI 线程: recv() → 渲染          │
└────────────┬───────────────┘
             │ TrayHandle（Clone + Send）
             │ 方法调用 / 事件通道
┌────────────┴───────────────┐
│           tray 库             │
│  前端: TrayHandle             │
│  后端: 内部事件线程（zbus 执行器） │
│   ├─ Watcher 跟踪/补位 (C1,C9) │
│   ├─ Item 属性同步     (C3,C4) │
│   ├─ DBusMenu 客户端      (C8) │
└────────────┬───────────────┘
             │ DBus session bus
     ┌───────┴────────┐
     │ StatusNotifierWatcher │
     │  (外部的或本库补位的)    │
     └────────────────┘
```

- 库启动后自持一条会话总线连接与一个内部事件线程；消费方只在 UI 线程持有 `TrayHandle`；
- **回调契约**：所有回调在库的内部事件线程上**串行**触发，消费方回调内不得阻塞、不得直接调用 GTK；需要进 UI 线程时由消费方自行桥接（如 `glib::MainContext::invoke`）；
- **通道契约**：默认封装。内部以回调原语驱动一个 `std::sync::mpsc::Sender`，消费方在任意线程 `recv()`。通道缓冲上限后投递失败仅记录告警，不阻塞库线程。

---

## 5. 公开接口

以下为公开 API 的**签名规格**（Rust 草图）。命名与签名一旦发布遵循语义化版本，v0 阶段允许按本文档修订。

### 5.1 入口与配置

```rust
pub struct TrayConfig {
    /// 服务发现与 Watcher 让位等待基准时长（ms），默认 2000。
    /// 外部 Watcher 死亡后等待 2×timeout 才补位。
    /// 注意：DBus 单次方法调用超时由 zbus 默认值承担，本参数不约束。
    pub timeout_ms: u64,
    /// 属性变更合并窗口，默认 50 ms（见 §6.2 去抖）
    pub debounce_ms: u64,
    /// 菜单拉取最大深度，默认 8
    pub menu_max_depth: u8,
    /// Fallback Watcher 策略，默认 Auto
    pub fallback: FallbackPolicy,
}

pub enum FallbackPolicy {
    /// 无外部 Watcher 则补位；外部 Watcher 出现则让位（默认）
    Auto,
    /// 永不补位，仅作为 Host/Item 客户端存在
    Never,
}

pub struct Tray;

impl Tray {
    /// 连接会话总线并启动内部线程；阻塞至完成 Host 注册（或超时）
    pub fn connect(config: TrayConfig) -> Result<(TrayHandle, Receiver<TrayEvent>), TrayError>;

    /// 底层原语：以回调方式构建。返回的 TrayHandle 不带通道，
    /// 事件全部经回调投递。可与 subscribe() 叠加使用。
    pub fn builder(config: TrayConfig) -> TrayBuilder;
}

pub struct TrayBuilder { /* ... */ }

impl TrayBuilder {
    /// 注册事件回调。可多次调用，按注册顺序串行触发
    pub fn on_event(self, cb: impl Fn(&TrayEvent) + Send + 'static) -> Self;
    /// 组装并启动，语义同 Tray::connect
    pub fn spawn(self) -> Result<TrayHandle, TrayError>;
}
```

### 5.2 TrayHandle

```rust
#[derive(Clone)]
pub struct TrayHandle { /* Send + Sync + 'static */ }

impl TrayHandle {
    /// 当前全部 Item 的快照（深拷贝，读多写少场景下无锁争用顾虑）
    pub fn items(&self) -> Vec<TrayItem>;
    /// 按 Id 取单个 Item 快照
    pub fn item(&self, id: &TrayItemId) -> Option<TrayItem>;
    /// 取 Item 的菜单快照；该 Item 无菜单或未就绪时返回 None
    pub fn menu(&self, id: &TrayItemId) -> Option<MenuSnapshot>;

    // ── 交互（C7），全部为 fire-and-forget + 结果经事件回执 ──
    pub fn activate(&self, id: &TrayItemId, x: i32, y: i32) -> Result<(), TrayError>;
    pub fn secondary_activate(&self, id: &TrayItemId, x: i32, y: i32) -> Result<(), TrayError>;
    pub fn scroll(&self, id: &TrayItemId, delta: i32, orient: Orientation) -> Result<(), TrayError>;

    // ── 菜单交互（C8）──
    /// 触发菜单条目（等价向 dbusmenu 发送 "clicked" 事件）
    pub fn menu_activate(&self, id: &TrayItemId, menu_item_id: i32) -> Result<(), TrayError>;
    /// 通知 Item 某菜单即将展示（AboutToShow），返回是否建议刷新
    /// 同步语义：调用后立即重拉该 Item 的菜单快照（懒加载型
    /// DBusMenu 提供方只在收到 AboutToShow 后才填充布局），
    /// 随后读 menu() 即为最新内容；内容有变则投递 MenuChanged
    pub fn menu_about_to_show(&self, id: &TrayItemId, menu_item_id: i32) -> Result<bool, TrayError>;

    /// 订阅事件通道（独立于回调原语，可多订阅者）
    pub fn subscribe(&self) -> Receiver<TrayEvent>;

    /// 优雅停机：注销 Host、停内部线程；drop 时自动调用
    pub fn shutdown(&self);
}

pub enum Orientation { Horizontal, Vertical }
```

### 5.3 数据模型

```rust
/// 全局唯一键：总线唯一名 + 对象路径。
/// 同一应用重启后唯一名变化 → 视为不同 Item（旧的 Removed、新的 Added）。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TrayItemId {
    pub bus_name: String,     // 如 ":1.42" 或 "org.example.App"
    pub path: String,         // 如 "/StatusNotifierItem"
}

pub struct TrayItem {
    pub id: TrayItemId,
    pub category: TrayCategory,       // Application / Communications / SystemServices / Hardware
    pub status: TrayStatus,           // Passive / Active / NeedsAttention
    pub title: Option<String>,
    pub description: Option<String>,  // SNI "Id" 属性，人类可读标识
    pub window_id: i32,
    pub icon: IconSource,             // 主图标（含解析规则）
    pub overlay_icon: Option<IconSource>,
    pub attention_icon: Option<IconSource>,
    pub tooltip: Option<ToolTip>,
    pub has_menu: bool,               // Menu 属性是否指向有效 dbusmenu 对象
}

pub enum TrayCategory { Application, Communications, SystemServices, Hardware }
pub enum TrayStatus   { Passive, Active, NeedsAttention }

/// 图标二选一规则：IconName 非空则 Name 优先（交由消费方查主题），
/// 否则交付位图。规则固定，消费方无需自行判断。
pub enum IconSource {
    Name(String),
    Pixmap(Pixmap),
}

/// SNI IconPixmap：ARGB32，网络字节序，行跨度 = width * 4（规范值）。
/// 本库不做色彩空间转换；转 RGBA 是消费方/UI 层的事。
pub struct Pixmap {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u8>,   // len == (width * height * 4)
}

pub struct ToolTip {
    pub icon: Option<IconSource>,
    pub title: String,
    pub description: String,
}
```

### 5.4 菜单模型

```rust
/// 只读菜单快照。每次 MenuChanged 事件后重新调用 menu() 获取新快照。
pub struct MenuSnapshot {
    pub revision: u32,
    pub root: Vec<MenuItem>,   // 顶层条目，深度截断于 TrayConfig::menu_max_depth
}

pub struct MenuItem {
    pub id: i32,                      // dbusmenu 条目 id
    pub kind: MenuItemKind,           // Standard / Separator / 容器（有子树）
    pub label: String,                // 已解析 &
    pub enabled: bool,                // 灰置
    pub visible: bool,
    pub toggled: Option<bool>,        // toggle-type 为 checkmark/radio 时有值
    pub icon: Option<IconSource>,
    pub children: Vec<MenuItem>,
}

pub enum MenuItemKind { Standard, Separator, Submenu }
```

菜单同步策略：库在收到 `LayoutUpdated` / `ItemsPropertiesUpdated` / `AboutToShow` 后**重拉受影响子树的布局与属性**，组装为全新 `MenuSnapshot`，对消费方只投递一个 `MenuChanged` 事件。消费方永不接触 dbusmenu 协议细节。

---

## 6. 事件模型

### 6.1 TrayEvent

```rust
pub enum TrayEvent {
    /// 新 Item 出现（含库启动时对存量 Item 的补发）
    Added(TrayItem),
    /// Item 消失（注销、进程退出、Watcher 更替导致失联）
    Removed(TrayItemId),
    /// 属性变更。changed 列出实际变化的字段（去抖合并后的并集）
    Changed { id: TrayItemId, changed: Vec<ItemField>, item: TrayItem },
    /// 该 Item 的菜单内容变化，需重新拉取快照
    MenuChanged(TrayItemId),
    /// Watcher 状态变化（外部 Watcher 出现/消失、本库补位/让位）
    WatcherChanged(WatcherState),
    /// 某次交互调用的异步回执（成功或 Item 侧错误）
    InteractionResult { id: TrayItemId, op: InteractionOp, result: Result<(), String> },
    /// 库内部不可恢复错误（如总线断开），事件投递后库停止工作
    Fatal(String),
}

pub enum ItemField {
    Status, Title, Description, Icon, OverlayIcon,
    AttentionIcon, Tooltip, WindowId, Category, HasMenu,
}

pub enum WatcherState {
    ExternalUp,      // 外部 Watcher 在线
    ExternalDown,    // 外部 Watcher 离线（且本库未补位）
    FallbackActive,  // 本库补位 Watcher 生效中
}

pub enum InteractionOp { Activate, SecondaryActivate, Scroll, MenuActivate, MenuAboutToShow }
```

### 6.2 投递语义

| 语义 | 规定 |
| :--- | :--- |
| 顺序 | 同一 Item 的事件严格有序；全局不保证跨 Item 顺序 |
| 去抖 | `PropertiesChanged` 在 `debounce_ms` 窗口内合并为一次 `Changed`，`changed` 字段取并集 |
| 补发 | `Added` 之外的存量语义：启动扫描时逐个投递 `Added`，与运行期新增无区别 |
| 背压 | 通道满时丢弃并记录告警（计数器），**绝不**阻塞内部线程 |
| 终止 | `Fatal` 是最后一个事件；此后 `TrayHandle` 全部方法返回 `TrayError::Stopped` |

---

## 7. Fallback Watcher（C9）

| 场景 | 行为 |
| :--- | :--- |
| 启动时无 Watcher | 库注册 `org.kde.StatusNotifierWatcher`（实现 `RegisterStatusNotifierItem`、`RegisterStatusNotifierHost`、`IsStatusNotifierHostRegistered`、`RegisteredStatusNotifierItems`、`StatusNotifierItemRegistered/Unregistered` 信号），事件投递 `WatcherChanged::FallbackActive` |
| 启动时有 Watcher | 仅作为 Host 登记，投递 `WatcherChanged::ExternalUp` |
| 运行中外部 Watcher 消失 | 等待 `timeout_ms`×2 确认不复原后补位，投递 `FallbackActive`；期间 Item 全部保留（多数 Item 会随 Watcher 更替信号自行重注册） |
| 运行中外部 Watcher 出现（本库补位中） | 让位：注销自身 Watcher 服务，向新 Watcher 重新登记 Host，投递 `ExternalUp`。已跟踪 Item 不销毁，等待各 Item 重注册信号做增量修正 |
| `FallbackPolicy::Never` | 永不补位，只投递 `ExternalDown` |

---

## 8. 错误模型

```rust
#[derive(Debug, thiserror::Error)]
pub enum TrayError {
    #[error("session bus connect failed: {0}")]
    Bus(String),
    #[error("operation timed out after {ms} ms")]
    Timeout { ms: u64 },
    #[error("item {0} not found")]
    ItemNotFound(String),
    #[error("dbus call failed: {0}")]
    Protocol(String),
    #[error("tray runtime stopped")]
    Stopped,
}
```

容错总则：

1. **库不 panic**。一切协议层异常（属性缺失、类型不符、Item 无响应）降级为：字段取默认值 / 方法返回 `Err` / `InteractionResult` 回执；
2. 单个 Item 的死亡不得影响其他 Item 的跟踪；
3. 总线断开 → 投递 `Fatal`，库进入 `Stopped`，由消费方决定重建。

---

## 9. 线程与生命周期

- `TrayHandle`: `Clone + Send + Sync + 'static`；所有方法线程安全；
- 内部固定两个自有线程：**事件线程**（信号分发、协议调用、事件投递的全部协议逻辑）与**定时器线程**（仅负责在去抖/重试截止时刻唤醒事件线程，不执行任何协议逻辑；zbus 阻塞消息迭代器不支持超时等待，故单列）；
- `shutdown()` 幂等；最后一个 `TrayHandle` 实例 drop 时自动触发 shutdown；
- 库不安装任何全局状态：同进程可并行运行多个 `Tray` 实例（不同 Watcher 策略下行为由 §7 约束）。

---

## 10. 二次开发者脚手架（交付规格）

库仓库必须随附以下脚手架，全部**只依赖本库公开 API**，作为二开者的测试与观察入口。以下为规格，随实现一同交付。

### 10.1 `tray-inspect`（CLI 观察工具，仓库 bin 目标）

```
tray-inspect list                 # 一次性打印全部 Item 快照表（id/状态/标题/图标/有无菜单）后退出
tray-inspect watch                # 实时事件流，逐行打印 TrayEvent（含时间戳），Ctrl-C 退出
tray-inspect menu <bus> <path>    # 打印指定 Item 的菜单树（缩进文本 + 可见/禁用/勾选标记）
tray-inspect activate <bus> <path> [x y]   # 触发 Activate 并打印回执
tray-inspect dump-json            # 全量快照（Item + 菜单）以 JSON 输出，供脚本化 diff
```

**验收**: 无需编写任何代码，`cargo run --bin tray-inspect -- watch` 即可观察托盘全生命周期；与 10.2 的 demo-item 联动可完成全部事件的复现。

### 10.2 `demo-item`（可编程测试替身，examples 目标）

一个假 SNI 应用：向总线注册一个 Item，并从 stdin 接受指令实时变更自身，用于**在没有真实托盘应用的环境**（CI、裸 Hyprland 会话）驱动库的全部代码路径。

```
stdin 指令集:
  status <passive|active|needs-attention>   # 切换状态
  icon <icon-name>                          # 切换主题图标
  pixmap                                    # 在主题图标与内嵌 8x8 位图间切换
  tooltip <title> <description>             # 设置 Tooltip；空参数清除
  menu add <label> / menu toggle <id> / menu remove <id>   # 菜单增删改
  toggle-menu                               # 挂载/卸载 Menu 对象
  remove                                    # 注销 Item 并退出
```

**验收**: 每条指令产生的协议变化均能被 `tray-inspect watch` 观察到对应事件。

### 10.3 `minimal.rs`（最小嵌入示例，examples 目标）

≤ 30 行：`Tray::connect` → `recv()` 循环打印事件 → Ctrl-C 退出。作为二开者复制粘贴的起点，并作为文档示例的权威来源。

### 10.4 `testing` 特性（`--features testing`）

公开 `tray::testing::spawn_demo_item(config) -> DemoItem`，供二开者在自己的测试中内嵌启动假 Item（10.2 的库形态复用），`Drop` 时自动注销。

---

## 11. 测试与验收

| 层级 | 内容 |
| :--- | :--- |
| 单元测试 | 事件去抖合并、`IconSource` 二选一规则、`TrayItemId` 相等语义 |
| 集成测试 | 同一进程内 `Tray(Auto)` + `testing::spawn_demo_item`：注册→属性变更→菜单变更→注销的全事件序列断言 |
| 互操作验收 | 与真实 SNI 提供方（如 `nm-applet`、`blueman-applet`）互通：`tray-inspect list` 可见、`watch` 无错误事件、`activate` 回执成功 |
| 无 Watcher 验收 | 裸 Hyprland 会话（无任何 Watcher）下 `tray-inspect watch` 首事件为 `WatcherChanged::FallbackActive`，demo-item 正常注册 |
| PRD 追溯 | 见 §12 映射表，逐条可验收 |

CI 最低门禁：`cargo build`、`cargo test`（含集成测试）在无显示服务环境下通过（集成测试不需要 Wayland/X11，仅需 dbus session）。

---

## 12. PRD 需求追溯

| PRD §3.3 需求 | 库能力 | 落点 |
| :--- | :--- | :--- |
| 动态检测应用注册/注销 SNI 服务 | C3、C9 | `TrayEvent::Added/Removed`、§7 |
| 支持图标加载 | C5 | `TrayItem::icon`、`IconSource`/`Pixmap`（渲染在 UI 层） |
| 支持 Tooltip 显示 | C6 | `TrayItem::tooltip` |
| 左键交互（Activate） | C7 | `TrayHandle::activate` |
| 右键交互（ContextMenu） | C8 | `menu()` + `MenuSnapshot` + `menu_activate`（菜单渲染在 UI 层） |
| 图标状态实时同步（Phase 2） | C4 | `TrayEvent::Changed`（含 `ItemField::Status`） |
| 高对比度边框/深浅色智能选择 | Non-goal | UI 层职责；本库交付原始像素（§1.2） |

---

## 13. 版本与稳定性

- 当前版本 **v0.x**：本文档为接口冻结基线；任何破坏性变更必须先修订本文档并升版本号；
- v1.0 冻结条件：§11 全部验收项通过 + 一个真实桌面会话连续运行 72h 无 `Fatal`；
- 本文档修订历史记录于文末。

---

## 修订历史

| 版本 | 日期 | 说明 |
| :--- | :--- | :--- |
| v0.1 | 2026-09-10 | 初版规格：能力清单、公开接口、事件模型、脚手架与验收定义 |
| v0.2 | 2026-09-11 | PoC 实现（zbus 5 blocking API）：①§9 线程模型修订为事件线程 + 定时器线程（阻塞迭代器无超时能力）；②`timeout_ms` 语义收窄为发现/让位等待，DBus 调用超时由 zbus 默认承担；③集成测试固定跑在私有 dbus-run-session 中以保证 Fallback 断言确定性 |
| v0.3 | 2026-09-11 | `menu_about_to_show` 语义增强：AboutToShow 之后同步重拉菜单快照（DBusMenu 懒加载提供方仅在 AboutToShow 后填充布局；修复真实应用右键菜单为空/回退 SecondaryActivate 的问题） |
