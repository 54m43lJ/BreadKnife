# 提示词文档：BreadPan / Boulanger 文档体系生成任务
## 第 0 部分：任务说明（给 agent 的开场指令）
你要为一个 Linux 系统托盘工具链创建四份规格文档。工具链包含两个库 crate 和一个下游验收工具，全部文档为**中文正文、代码标识符与 API 名英文**。你的产出只有文档，不写代码。
文档生成的最高准则：
1. **每一条约束都要能回答"为什么"**——写不出理由的约束删掉；
2. **实现细节不进正文**——线程拆分、字节序策略等工程决策写入各文档的「架构决策记录（ADR）」附录，每条 3–5 行；
3. **示例代码即文档**——所有代码示例必须是与公开 API 签名逐字一致的可编译草图，并标注"将作为 doctest 保真维护"；
4. **零下游词汇**——breadpan 与 boulanger 的文档中不得出现任务栏/面板/panel/taskbar/BreadKnife 等任何消费方概念；消费方用法只允许出现在 demo-tray/README.md，且引用方向单向（demo-tray → 两库）；
5. 图表一律 ASCII 字符画。
---
## 第 1 部分：全局决策记录（所有文档的事实基础，不得偏离）
### 1.1 项目身份
| 项 | 决策 |
|---|---|
| 项目 | 面包系工具链：`breadpan`（托盘客户端库）+ `boulanger`（SNI Watcher 角色库） |
| 命名逻辑 | breadpan=烤盘（承放、供取用）；boulanger=法语面包师（照看炉火、收发面团＝登记 Item） |
| 发布状态 | 暂不发布至 crates.io；版本策略照常生效（见 1.6） |
| 使用场景 | 钉死 Wayland 会话 |
| 目录形态 | 同一 workspace 下平行 crate；`breadpan ⊥ boulanger`，互相零依赖 |
### 1.2 breadpan 定位与边界
**是什么**：无头（headless）的 SNI（StatusNotifierItem）协议客户端库 + DBusMenu 客户端库，以统一事件流交付托盘世界的全部变化。**不实现 Watcher 角色**（Watch 角色由 boulanger 承担；breadpan 仅作为客户端观察 Watcher 存亡）。
**Non-goals（写明"为什么"）**：

| 不做 | 理由 |
|---|---|
| 任何 UI | headless 是第一设计原则；渲染、主题查找、样式归消费方 |
| XEmbed/X11 传统托盘 | 使用场景钉死 Wayland；SNI 是 Wayland 时代协议 |
| 菜单渲染/弹出定位 | 交付 MenuSnapshot 数据模型与激活接口即可 |
| 图标主题解析 | 只交付 `IconSource`，主题查询归消费方 |
| AttentionMovieName | 实际使用者寥寥，进「已知不实现清单」 |
| Fallback Watcher 补位 | Watcher 是独立角色，由 boulanger 承担；库内补位在生产路径是死代码且引发总线名争抢 |
| 自动字节序纠偏 | 见待定决策 D1 |
| 异步运行时泄漏 | 公开 API 零 async；内部用 zbus 自带执行器 |
### 1.3 依赖白名单（硬约束）
`zbus`（锁 `"5"`，允许 patch 升级）+ `thiserror` + `std`。**不引入日志门面**——日志是消费方的职责，库通过事件流汇报自身状态（见 1.5）。workspace 内零反向依赖。
### 1.4 耦合硬约束（可检验）
- **a. workspace 零反向依赖**：两库 Cargo.toml 不出现 workspace 内任何成员；CI 在隔离目录 `cargo build -p breadpan` / `-p boulanger` 验证；
- **b. 零下游概念泄漏**：公开 API 命名、类型、doc comment 不得含任务栏词汇；检验标准：`cargo doc` 页面给从未见过下游项目的人可无障碍使用。
### 1.5 运行模型（关键语义，正文必须完整呈现）
**连接时序**：
```
connect(config) / builder().spawn()      ← 二者语义完全相同，connect 是 sugar
  ├─ 连接会话总线            ← 唯一同步失败点，失败立即 Err(Bus)，不重试
  ├─ 启动内部线程，立即返回 (TrayHandle, Receiver<TrayEvent>)   ← 纯非阻塞
  └─ 后台：Watcher 发现
        ├─ 无 Watcher → 重试：500ms 起指数退避（×2），封顶 30s，无限重试
        │     每次 timeout 到点 → 投递 Internal(Info) 汇报（含第几次尝试、累计等待时长）
        ├─ Watcher 出现 → Host 注册 → WatcherChanged(ExternalUp)
        └─ 启动扫描 → 逐个补发存量 Item 的 Added（与运行期新增无区别）
```
**两条生命线**（正文必须显式区分，这是防混乱的关键）：

| 信息类型 | 载体 | 例子 |
|---|---|---|
| 机器可读的状态变迁 | SNI 事件（`WatcherChanged` / `Added` / …） | Watcher 上线 |
| 人类可读的过程叙述 | `Internal(Info/Warn)` | "watcher 未就绪，第 3 次重试，已等待 2.0s" |
| 终止宣告 | `Internal(Fatal)` | 总线断开；此后 handle 全部方法返回 `Err(Stopped)` |

**回调契约**：所有回调在库内部专用线程上串行触发；回调内不得阻塞、不得直接调用 GTK；进 UI 线程由消费方自行桥接。
**通道契约**：内部以回调原语驱动默认 `std::sync::mpsc` 通道；通道满时丢弃并累计（`dropped_event_count()` 计数器暴露），**绝不阻塞内部线程**（通道满时不能投递"通道满"事件——自我矛盾，故用计数器，此取舍写入 ADR）。
**线程模型**：公开文档只承诺"回调串行于单一专用线程、方法线程安全、shutdown 幂等、最后一个 handle drop 自动 shutdown、可多实例并行"；内部"事件线程 + 定时器线程"拆分（zbus 阻塞迭代器无超时能力所致）写入 ADR。
### 1.6 版本与稳定性
- SemVer，但"破坏性变更"定义收窄为**"导致下游消费代码需要修改的变更"**；文档措辞调整、错误 enum 加 variant、`InternalMessage` 加结构化字段等不算；
- 不设 MSRV 承诺，文档记录当前验证通过的 Rust 版本；
- 公开 API 强制 `#![deny(missing_docs)]`；
- v1.0 冻结条件：全部验收项通过 + 真实桌面会话 72h 无 Fatal。
### 1.7 与 GTK 集成的文档口径
**不定义"权威模式"**。两种模式为一等公民并列呈现（对照表 + 各自完整示例代码），选择权在消费方：

| | 回调 + `MainContext::invoke` | 回调/通道 + 转发线程 + `glib::MainContext::channel` |
|---|---|---|
| 路径 | 库线程 → glib idle 队列 | 库线程 → mpsc → glib source |
| 适合 | 事件量小、处理简单 | 事件量大、需批处理/过滤/保留 mpsc 语义 |
### 1.8 交互调用的范式论证（正文必须收录此论证）
**分界规则：状态靠推送，控制靠有界同步往返。**

| 调用面 | 性质 | 范式 |
|---|---|---|
| Item 生命周期/属性/菜单内容 | 状态流（可能含大 Pixmap） | 推送（事件通知）+ 消费方主动拉快照 |
| `activate` / `secondary_activate` / `scroll` / `menu_activate` | 控制指令，无返回值 | 同步 fire-and-forget，返回 `Result<(), TrayError>` |
| `menu_about_to_show` | 控制指令，答案在渲染前必需 | 同步往返，返回 `Result<bool, TrayError>` |

论证要点（写入正文）：同步往返典型 0.5–5ms，远低于人感知阈值；异步化会使右键弹窗延迟抖动并引入 GLib 侧状态机复杂度；真实风险是坏 Item 挂死导致 DBus 默认 25s 超时——由 `call_timeout_ms`（默认 **500ms**，数值推导：人急躁时反复右键 → 挂起须近无感，300ms 即够、留弱硬件/虚拟机余量）钉死上界，超时返回 `Err(Protocol)`。该数值推导理由写入 ADR，防止未来的自己"优化"它。
---
## 第 2 部分：文件清单与目录树
```
<workspace root>/
├── README.md              # 一页纸：三 crate 一览 + 依赖图 + 阅读入口
├── breadpan/README.md     # 主规格文档（本任务的核心产出，篇幅最大）
├── boulanger/README.md    # Watcher 角色规格（独立小库，独立成文）
└── demo-tray/README.md    # 人工验收/debug 工具文档（唯一允许写两库用法的下游文档）
```
**依赖与引用方向**（写入根 README 的图）：
```
breadknife (下游，不在本仓库)
    ├─ 先启动 boulanger 服务，再 breadpan::connect
    │
demo-tray ──依赖──> breadpan + boulanger   （用法文档仅在此）
```
---
## 第 3 部分：逐文档内容明细表
### 3.1 `README.md`（根，一页纸）
| 章节 | 必写内容 |
|---|---|
| 项目一览 | breadpan / boulanger / demo-tray 各一句话定位 |
| 依赖图 | ASCII 图：两库互相零依赖，demo-tray 单向依赖两库，下游消费方组装两者 |
| 阅读入口 | 指向三份子文档；注明"库文档不含任何下游项目信息" |
| 状态 | v0.x，未发布，接口冻结基线，破坏性变更须先改文档 |
### 3.2 `breadpan/README.md`（主规格，按以下章节顺序）
**§0 30 秒了解**
- 一句话定位；`minimal.rs` 全文（≤30 行：connect → recv 循环打印事件 → Ctrl-C），标注为 doctest 保真与文档权威示例来源。
**§1 定位与设计原则**
- 是什么 / 不是什么（按 1.2 Non-goals 表，每条带理由）；
- 设计原则五条，每条附"为什么"：① headless；② 阻塞优先（零 async 泄漏）；③ 状态推送 + 主动拉快照（不推大对象）；④ 容错降级（永不 panic，坏 Item 不拖垮整体，单 Item 死亡不影响其他 Item）；⑤ 依赖白名单（zbus/thiserror/std）；
- 硬约束：Wayland-only、仅 SNI、workspace 零反向依赖、零下游词汇（检验标准）。
**§2 快速上手**
- 最小示例（§0 同源）；
- GTK/GLib 集成：按 1.7 对照表 + 两种模式完整代码。
**§3 架构概览**
- ASCII 图：消费方线程 / breadpan 内部线程 / session bus / Watcher（外部的或 boulanger 提供的）；
- 连接时序图（按 1.5）；重试曲线（500ms/×2/封顶 30s/无限，理由：线性递增在 Watcher 长期缺席时变成无意义总线流量）；
- 线程与生命周期契约（公开承诺版，按 1.5）。
**§4 公开 API 参考**（逐项含语义说明；签名以下方 §5 草图为准，逐字一致）

| 小节 | 必写要点 |
|---|---|
| 4.1 入口与配置 | `TrayConfig` 各字段默认值与语义（timeout_ms=500 为 Watcher 发现重试基准；debounce_ms=50；menu_max_depth=8；call_timeout_ms=500）；`connect` 与 `builder().spawn()` 语义相同（sugar 关系）；均纯非阻塞 |
| 4.2 TrayHandle | 全方法语义；**subscribe() 必须交付初始状态快照**（先补发当前 Watcher 状态、再补发存量 Added，然后进入实时流——与启动扫描语义同构，规则统一）⏳；`dropped_event_count()`；`shutdown()` 幂等 |
| 4.3 数据模型 | `TrayItemId`（bus_name + path；同一应用重启后唯一名变化 = 不同 Item；**跨重启稳定关联建议用 SNI `Id` 属性**——必须写，否则每个使用者都会重踩）；`TrayItem` 全字段（含 `item_is_menu`：该 Item 左键亦弹菜单而非 Activate，消费方自行决定左键行为）；`IconSource` 二选一规则（Name 非空则 Name 优先）；`Pixmap`（ARGB32、规范字节序、stride=width×4，库不转换）；`ToolTip` |
| 4.4 菜单模型 | `MenuSnapshot`/`MenuItem`；**`menu_activate` 的 item id 仅在当前 MenuSnapshot 生命周期内有效，布局更新后 id 可能复用**（醒目警告）；label 的 mnemonic 处理口径 ⏳（D3）；菜单同步策略：收到 LayoutUpdated/ItemsPropertiesUpdated/AboutToShow 后重拉受影响子树组装全新快照，对消费方只投递一个 MenuChanged；AboutToShow 强制刷新兜底的理由（部分 Item 从不发 LayoutUpdated） |

**§5 事件语义**
- `TrayEvent` 全变体及投递语义表：同一 Item 严格有序 / 全局跨 Item 不保证；去抖合并（debounce 窗口内 PropertiesChanged 合并为一次 Changed，changed 字段取并集）；存量补发；背压丢弃 + 计数器；`Internal(Fatal)` 终止后全部方法返回 `Stopped`；
- **事件定稿**（原 SPEC 的修订点须在正文体现）：`Changed` 不携带快照（只带 id + changed 字段列表，消费方 `item(id)` 拉取——与 MenuChanged 语义统一，避免深拷贝 Pixmap）；`InteractionResult` 删除（同步 Result 取代）；`Fatal(String)` 删除（并入 `Internal(Fatal)`）；`WatcherChanged` 仅 `ExternalUp/ExternalDown`（`FallbackActive` 删除）；
- ExternalDown 的客户端韧性语义：保留已知 Item、暂停发现、Watcher 回来后增量修正（探测策略 ⏳ D2）。
**§6 协议实现说明**
- 6.1 SNI 覆盖范围：全部属性清单（含 ToolTip、ItemIsMenu）、交互调用；已知不实现清单（AttentionMovieName；XEmbed）；
- 6.2 DBusMenu 覆盖范围：布局拉取、增量同步、AboutToShow 策略、menu_max_depth 截断；
- 6.3 已知协议偏差与不保证清单 ⏳（D1 内容）：真实 Item 的字节序偏差现象与肉眼识别方法；IconName 主题查不到时的信息损失说明；
- 6.4 Host 注册细节：唯一名 `_org.kde.StatusNotifierHost.<pid>-<序号>`，同 pid 多实例序号分配规则。
**§7 测试与脚手架**
- 7.1 `testing` 特性（`--features testing`）：`tray::testing::spawn_demo_item(config) -> DemoItem`（Drop 自动注销）、`tray::testing::DemoWatcher`（demo 级 Watcher 替身，take/release 控制持有）；**定位声明：测试替身而非生产实现，与 boulanger 不构成重复**——替身保证 breadpan 在裸会话下的 CI 自足性；
- 7.2 仓库脚手架：`tray-inspect`（CLI：list / watch / menu / activate / dump-json，逐条验收标准）；`demo-item`（stdin 指令集逐条列出；裸会话可用性靠 DemoWatcher 自动挂载）；`minimal.rs`；`demo-watcher-switch`（回车切换 Watcher 名字持有/释放，对运行中的 Host 制造 WatcherChanged 变迁）；
- 7.3 CI 门禁：`cargo build`、`cargo test`（含集成测试）在无显示服务环境通过；集成测试固定跑在私有 `dbus-run-session` 中保证确定性；测试仅需 dbus session，不需要 Wayland/X11。
**§8 错误模型**
- 四变体定稿：`Bus(String)` / `Protocol(String)` / `ItemNotFound(String)` / `Stopped`；**`Timeout` 变体已删除**（重试进展走 Internal 消息，不再有超时错误）；容错总则三条（不 panic；单 Item 死亡隔离；总线断开 → Internal(Fatal) → Stopped，由消费方决定重建）。
**§9 验收标准**
- 单元测试项：事件去抖合并、IconSource 二选一、TrayItemId 相等语义；
- 集成测试项：同进程 spawn_demo_item 全事件序列断言（注册→属性变更→菜单变更→注销）；
- 互操作验收：与真实 SNI 提供方（nm-applet、blueman-applet）互通；
- 无 Watcher 验收：裸会话下重试日志（Internal Info 序列）可见、demo-item 注册后正常工作。
**附录 A：架构决策记录（ADR）**——每条 3–5 行，含背景/决策/理由：
① 定时器线程独立（zbus 阻塞迭代器无超时能力）；② Fallback Watcher 移除（生产路径死代码 + 总线名争抢，Watch 角色归 boulanger）；③ 重试曲线 500ms/×2/30s 推导；④ call_timeout_ms=500ms 推导；⑤ 事件不携带快照；⑥ 通道满用计数器而非事件（自指矛盾）；⑦ InternalMessage 以 String 起步（程序化需求出现时加字段是非破坏性变更）；⑧ Fatal 并入 Internal(Fatal)（生命线分离）；⑨ Timeout 错误变体删除（错误是方法结果，重试叙述是事件）。
**附录 B：版本与稳定性**（按 1.6）。
### 3.3 `boulanger/README.md`（Watcher 角色规格）
| 章节 | 必写内容 |
|---|---|
| 定位 | SNI Watcher 角色的纯服务端小库；无 UI；运行形态 = 独立 crate、由消费方进程内组装（留后门：加 main() 即可独立常驻进程，理由：Watcher 活得比面板久可避免面板重启引发的 Item 重注册风暴）；规模预期 300–500 行 |
| 与 breadpan 的正交性 | 显式声明：breadpan ⊥ boulanger，互不依赖；breadpan 的 DemoWatcher 是自带测试替身，不构成重复 |
| 协议面 | 持有 `org.kde.StatusNotifierWatcher` 总线名；4 方法（RegisterStatusNotifierItem/Host、IsStatusNotifierHostRegistered、RegisteredStatusNotifierItems）；2 信号（ItemRegistered/Unregistered）；NameOwnerChanged 时清理死名字的簿记；名字带 AllowReplacement |
| 行为表 | Item/Host 注册与注销；唯一名消失清理；对 AllowReplacement 的让位语义 |
| 组装示例 | 消费方进程内"先 boulanger 服务、再 breadpan::connect"的时序代码草图（不点名任何具体下游项目） |
| 验收 | dbus-run-session 内独立测试；与 breadpan 集成测试联动 |
| ADR 附录 | 独立 crate 而非并入客户端库的理由；同进程组装 vs 独立进程的取舍 |
### 3.4 `demo-tray/README.md`（人工验收/debug 工具）
| 章节 | 必写内容 |
|---|---|
| 身份声明 | 外围人工验收与调试工具；本仓库内**唯一**展示两库用法的文档；不是库文档的一部分 |
| 功能 | GTK4 简单窗口：卡片渲染 IconSource（主题名缺省回退 image-missing；Pixmap 转 RGBA）；左键 activate（含 item_is_menu 时的菜单行为）、滚轮 scroll、右键按 menu() 快照弹菜单（先 menu_about_to_show）；底部事件日志（含 Internal 消息展示，体现"客户端自行管理库叙述流"）；内置测试台：spawn demo item 并驱动全部变化、take/release watcher 角色 |
| 运行 | `cargo run -p demo-tray` |
| 验收 | 纯 Hyprland（无任何托盘环境）下从窗口内复现全部事件类型 |
---
## 第 4 部分：API 签名定稿草图（agent 逐字对齐基准）
```rust
pub struct TrayConfig {
    pub timeout_ms: u64,      // Watcher 发现重试基准，默认 500，指数退避 ×2 封顶 30s
    pub debounce_ms: u64,     // 属性变更合并窗口，默认 50
    pub menu_max_depth: u8,   // 默认 8
    pub call_timeout_ms: u64, // 交互调用内部上界，默认 500
}
impl Tray {
    /// 非阻塞。连接会话总线（失败立即 Err(Bus)），后台执行 Watcher 发现与 Host 注册。
    pub fn connect(config: TrayConfig)
        -> Result<(TrayHandle, Receiver<TrayEvent>), TrayError>;
    pub fn builder(config: TrayConfig) -> TrayBuilder;  // on_event() 若干次后 spawn()，返回值同上
}
impl TrayHandle {  // Clone + Send + Sync + 'static
    pub fn items(&self) -> Vec<TrayItem>;
    pub fn item(&self, id: &TrayItemId) -> Option<TrayItem>;
    pub fn menu(&self, id: &TrayItemId) -> Option<MenuSnapshot>;
    // 交互：同步，受 call_timeout_ms 上界约束
    pub fn activate(&self, id: &TrayItemId, x: i32, y: i32) -> Result<(), TrayError>;
    pub fn secondary_activate(&self, id: &TrayItemId, x: i32, y: i32) -> Result<(), TrayError>;
    pub fn scroll(&self, id: &TrayItemId, delta: i32, orient: Orientation) -> Result<(), TrayError>;
    pub fn menu_activate(&self, id: &TrayItemId, menu_item_id: i32) -> Result<(), TrayError>;
    pub fn menu_about_to_show(&self, id: &TrayItemId, menu_item_id: i32) -> Result<bool, TrayError>;
    /// 追加订阅；交付初始状态快照（Watcher 状态 + 存量 Added）后进入实时流
    pub fn subscribe(&self) -> Receiver<TrayEvent>;
    pub fn dropped_event_count(&self) -> u64;
    /// 幂等；最后一个 handle drop 自动触发
    pub fn shutdown(&self);
}
pub enum TrayEvent {
    Added(TrayItem),
    Removed(TrayItemId),
    Changed { id: TrayItemId, changed: Vec<ItemField> },   // 无快照，消费方 item(id) 拉取
    MenuChanged(TrayItemId),
    WatcherChanged(WatcherState),                           // 仅 ExternalUp / ExternalDown
    Internal(InternalMessage),                              // 库叙述流；Fatal 级 = 终止
}
pub struct InternalMessage { pub level: InternalLevel, pub message: String }
pub enum InternalLevel { Info, Warn, Fatal }
pub enum TrayError { Bus(String), Protocol(String), ItemNotFound(String), Stopped }
pub struct TrayItem {
    pub id: TrayItemId,            // bus_name + path
    pub category: TrayCategory,
    pub status: TrayStatus,
    pub title: Option<String>,
    pub description: Option<String>,  // SNI "Id"，跨重启稳定关联的推荐键
    pub window_id: i32,
    pub item_is_menu: bool,
    pub icon: IconSource,             // Name 非空则 Name 优先，否则 Pixmap
    pub overlay_icon: Option<IconSource>,
    pub attention_icon: Option<IconSource>,
    pub tooltip: Option<ToolTip>,
}
// MenuSnapshot / MenuItem / IconSource / Pixmap / ToolTip 沿用原规格 §5.3–5.4，
// MenuItem.label 口径见待定决策 D3；MenuItemKind { Standard, Separator, Submenu }
```
---
## 第 5 部分：agent 硬约束与完成定义
**硬约束**：
1. 中文正文、API/代码英文；2. 每条约束附"为什么"；3. 实现细节只进 ADR 附录；4. 代码示例与第 4 部分签名逐字一致并标注 doctest 保真；5. breadpan/boulanger 文档零下游词汇（生成后自查关键词：taskbar/panel/taskbar/dock/BreadKnife/任务栏/面板）；6. ASCII 图表；7. 引用方向单向；8. 各文档间不复制内容，差异点互相引用。
**完成定义（agent 自检清单）**：
- [ ] 四份文件齐全，路径与第 2 部分一致
- [ ] 第 1 部分每条决策在对应文档中可定位到落点
- [ ] 第 4 部分 API 草图与 breadpan 文档 §4 完全一致（含事件变体、错误变体、字段名）
- [ ] 原 SPEC 的全部修订点已体现且无残留旧语义：FallbackPolicy/WatcherChanged::FallbackActive/InteractionResult/Fatal(String)/TrayError::Timeout/has_menu/Changed 携带快照——均为删除或替换后状态
- [ ] 三条"生命线"分工表、两种 GTK 桥接模式对照表、范式论证表均已收录
- [ ] ADR 附录覆盖第 3.2 节列出的 ①–⑨ 全部条目
- [ ] demo-tray 文档外的所有文件通过零下游词汇自查
- [ ] 待定决策区（第 6 部分）已由用户清空，无"D1/D2/D3"字样残留在任何产出文档中
---