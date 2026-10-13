# breadpan 实现记录

> 定位：ARCHITECTURE / CODE 两档**未覆盖**内容的补充记录——协议生态的实测怪癖、
> 文档契约留下缺口处的补全决策、落地时受技术栈约束的绕行方式。
> 条目内容升格入两档后**暂不删除**，待确认完工后统一清理。
> 权威边界不变：公开 API、事件与错误语义以
> [ARCHITECTURE.html §3](ARCHITECTURE.html#sec-3) 为准；内部骨架以
> [CODE.html](CODE.html) 为准。本文与二者冲突时，以二者为准。

## 1. 协议生态实测发现（文档未定义的输入形态）

实测环境：无 Watcher 的裸 Wayland 桌面 + 真实应用（QQ 音乐、clash-verge）。

| # | 现象 | 实例 | 本库处理 |
|---|------|------|----------|
| 1 | `RegisterStatusNotifierItem` 参数存在**三种形态**：知名名；`"name/path"` 混串；Ayatana 风格**纯对象路径** | clash-verge 传 `/org/ayatana/NotificationItem/...` | 前两种就地解析；纯路径形态取**调用方唯一名**作服务名（经消息头 sender 提取，KDE Watcher 的既定语义）；无 sender 或空串 → 拒绝受理（Protocol） |
| 2 | 存量 Item 可能**不实现 Introspect**（返回空 XML） | QQ 音乐（Electron/CEF 内核） | 反向发现探测双级：Introspect 接口名确认 → 失败则 `Properties.GetAll` 直接属性读取确认 |
| 3 | Item 可能以**唯一名**注册（无知名名） | clash-verge（`:1.22`） | 唯一名即 TrayItemId；`GetNameOwner` 解析属主（唯一名对自身解析成功） |
| 4 | `IconName` 可能是**文件路径**而非主题图标名 | clash-verge（`/run/user/1000/tray-icon/*.png`） | `IconName` 以 `/` 开头识别为 `IconSource::Path`（字面识别，不做语义猜测）；渲染直出加载文件位图，失败回退 `image-missing` |
| 5 | 同一对象可能**双重注册**（知名名 + 唯一名/路径形态各一次） | 确定性复现仅 mock。真实侧一例**疑似**：QQ 音乐（CEF）存量项以知名名在册的同时，出现另一条唯一名注册（未验证归属，应用退出后无法复查）。clash-verge 抓包实测**单次**注册（路径形态，且直发 Watcher 唯一名而非其知名名） | 设计为自愈：注册按字段匹配两轮 lookup（注册名 ↔ service、发送者唯一名 ↔ unique），命中原地更新——临时唯一名 id 在知名名报到时升级并发 NameChanged{old, new}（是否重载由消费方裁断）；两轮均未命中才新建条目（CODE §3.3.1） |
| 6 | `RegisterStatusNotifierHost` 兼容桩被真实 Host 调用 | vicinae-server（面板）在 Watcher 名出现时即调桩注册（本次抓包仅见调用，未捕获其是否读 `IsStatusNotifierHostRegistered`） | 桩语义按 §3.4：受理即忘、不簿记、不产生事件 |
| 7 | Item 可能**声明 Activate 却不实现**（libappindicator 系：XML 静态声明、分发 catch-all） | clash-verge（`No handler for Activate`，Context 连声明都没有） | 左键 Activate 非超时失败且 DBusMenu 在册 → 回退 Menu 流程（左键出菜单是宿主侧策略，菜单由宿主经 dbusmenu 渲染）；超时不回退（menu 与 Activate 同一主循环分发，同样不可读） |

## 2. 文档契约缺口的补全决策

ARCHITECTURE 规定了"首事件即快照、无重叠无缺口"，但未定义启动竞态的处理；以下为补全：

- **名字申请时序**：bootstrap 顺序为 挂载协议面 → 信号订阅 + 存活探测 → 反向发现
  → 快照（置位栅栏）→ 节流任务启动 → **最后申请 Watcher 名**（节流窗口只在快照后
  有意义，提前启动只是空转等栅栏）。名字在位前报到无从发生，所有报到天然落在
  快照之后、以增量事件衔接。（若按 CODE 伪码"先申请名"，等待 Watcher 出现的
  Item——如 clash-verge——其 `Added` 会抢在快照之前，实测复现。）
- **快照栅栏**（`snapshot_done`，防御性）：快照投递前置位；栅栏前的报到静默入册
  （由快照承载，不发 `Added`）；启动期 Item 死亡的 `Removed` 进缓冲，快照投递后
  仅对**快照内**的 id 补投（快照外的按从未出现丢弃）；`OwnerChanged` 栅栏前静默
  （重绑定照常生效，快照即最新，无需补投）；`MenuChanged` 在栅栏前丢弃；
  `Changed` 由节流任务保证不早于快照——其在栅栏置位后才启动（tokio interval
  首拍立即触发，启动期积压的 pending 首拍即冲刷）。
- **同名换主**（`NameOwnerChanged(name, old≠"", new≠"")`）：条目保留、id 不变——
  属主唯一名重绑定 + 路由字段就地重探，交付信息性 `OwnerChanged(id)`（属主更替对
  消费方无可感知影响，无必做动作；ARCHITECTURE §2.4/§3.2）。初版实现曾拆
  `Removed`+`Added`，代码对齐见 TODO.md。
- **宽容取值**（已升格：归一化行为 → ARCHITECTURE §3.3 字段表；变体级解析 →
  CODE §2.5，待确认清理）：规范外波动不 panic、按缺省处理——`WindowId` 以 I32 或 U32 送达
  均可解析；未知 `Category`/`Status` 回退缺省枚举；空 `Title` → `None`；
  `Menu` 属性以对象路径或字符串送达均可——统一经 `menu_path_of` 取值（两形态
  取文本后过 `ObjectPath::try_from` 规范校验，非字符串类变体或文本不合规 →
  `None`）；`PropertiesChanged` 路径解析失败**不覆盖旧值**——Menu 键被清空即
  路由键死亡：右键跌回 fallback、MenuChanged 永久静默（初版以 `TryFrom<Value>`
  钉类型、失败清空，代码对齐见 TODO.md）。

## 3. 关键落地细节（CODE 骨架未展开的取舍）

- **同步桥**（已升格：机制与禁令 → CODE §2.2/§3.5.1，待确认清理）：旧版 CODE
  `RuntimeShared.bus` 注释写"同步方法经 rt.block_on 受理指令"（be569df 引入，
  1b35341 去模糊化时移除），实际**不可行**——CurrentThread 运行时由内部线程驱动，
  跨线程并发 `block_on` 同一运行时属未定义行为。实现为 `rt.spawn`（命令 future）+
  `std::sync::mpsc` 阻塞等待。
- **等待通道选 std 而非 tokio**（已升格：任意线程可调承诺 → ARCHITECTURE §3.1；
  选型与理由 → CODE §2.2，待确认清理）：tokio `oneshot::Receiver::blocking_recv` 在
  消费方自己的异步上下文里调用会 panic；std 通道的 `recv_timeout` 无此限制
  （公开面承诺任意线程可调）。
- **超时层次**（已升格 / 同步桥守卫经裁决改为消费者可配置，待确认清理）：每个 DBus
  调用独立受 `timeout_ms` 约束（→ CODE 各调用栈）；`Tray::start` 引导总时限
  `max(10×timeout, 10s)`（→ CODE §3.1.1）。同步桥守卫不再由公式计算
  （4×timeout + 10s 经裁决废除——路径变化时静默失效），改为消费者可配置的
  `sync_guard_ms`（默认 30s，推荐 ≥ `timeout_ms`×4 + 10s）；守卫到期 →
  `TrayError::Hung`（ARCHITECTURE §3.1/§3.2、CODE §3.5.1），代码对齐见 TODO.md。
- **全局信号订阅**（6 条 match rule，对照 CODE 的"逐 Item 订阅"；已升格：空过滤行为 → CODE §3.1.1，待确认清理）：
  NameOwnerChanged、New\*×2 前缀、PropertiesChanged（arg0 限定）×2 前缀、
  dbusmenu。按 (sender 唯一名, path) 对照簿记过滤。免去逐 Item 加/删规则的
  簿记与竞态；代价是总线上未跟踪对象的信号被空过滤（可忽略）。
- **逐调用前缀回退**（已升格：前缀回退规则 → CODE §2.5/§3.5.2，待确认清理）：属性读取（GetAll）与交互调用（Activate/Scroll 等）先
  `org.kde` 失败再 `org.freedesktop`——兼容只导出规范原文拼写的少数实现；
  超时即止不重试（对端失联时另一前缀注定同耗）；双前缀俱败合并上报两侧
  错误，避免只报末次掩盖首侧真因。
- **菜单属性合并**（已升格：组属性覆盖规则 → CODE §3.4.1，待确认清理）：GetLayout 的布局内嵌属性为底，GetGroupProperties 的组属性
  覆盖之（专属性查询为权威来源）。
- **VOID 交互仍等待回执**（已升格：VOID 术语修正——无返回值、应答仍等待 → ARCHITECTURE §2.6/§3.2，待确认清理）：VOID 指无返回值，不指无应答——受理即返回
  `Dispatched`，应答/错误以 `InteractionResult` 事件交付（区分"送达"与
  "目标消失"）。
- **dbusmenu Event 参数**：`data` 变体发 `I32(0)`（服务端忽略），时间戳取
  Unix 毫秒截断 u32。
- **滚动符号**：up/left 为负、down/right 为正，steps 缺省 1 逐事件发送。
- **事件通道**：容量 256（库内定值）；满则丢弃并计数（有损交付）；关闭 =
  丢弃 Sender（消费端 `recv()` → `None`）。
- **心跳**：周期 5s（库内定值）；每 Item 探测 = `Get(Status)`；结论只写簿记
  `alive`，不产生事件。
- **自动停机的前提**：内部任务只持有子 Arc（tracker/events/pending/…），
  **不持有** `Arc<RuntimeShared>`——最后一个 Handle drop 时 strong_count==1
  才能触发 shutdown。
- **快照内排序**：按 id 排序投递（发现序不稳定，排序保证确定性输出）。

## 4. 技术栈约束与绕行（zbus 5 / tokio 1 / zvariant 5 / gtk4-rs 0.10）

| 约束 | 绕行 |
|------|------|
| CurrentThread 运行时跨线程并发 `block_on` 未定义 | 同步桥 spawn+std 通道（§3） |
| 依赖白名单不含 futures；MessageStream 需 Stream trait 消费 | `zbus::export::futures_core::Stream` + `std::future::poll_fn` + `std::pin::pin!` 手写 `next` |
| tokio mpsc Receiver 无 `recv_timeout` | try_recv 轮询 + 截止时间（start 等待与测试工具） |
| tokio mpsc/oneshot 的 Sender **非 Sync**（破坏 TrayHandle 契约） | 内部上报通道全部用 watch（Send+Sync） |
| zvariant `OwnedValue` 无 tuple 的 blanket 转换 | 复合类型经 `Value::from(OwnedValue)` 再 `try_into`（tuple 的 `TryFrom<Value>` 存在） |
| `OwnedValue` 是包装结构而非枚举，不能按变体匹配 | 经 `Deref` 到 `Value` 再匹配标量变体 |
| `StructureBuilder` 手工构造的结构回转 tuple 报 `IncorrectType`（实测）；`From<tuple> for Structure` 可靠 | 测试 mock 的布局节点一律用 `From<tuple>` 构造 |
| 名字申请必须走 `Connection` API（raw RequestName 会使 ObjectServer 不路由该名的入站调用） | `request_name_with_flags`，flags 为 enumflags2 `BitFlags`（`DoNotQueue.into()`） |
| zbus 警告：先申请名后挂协议面会丢早到调用 | 挂面 → 快照 → 申请名（与 §2 时序决策一致） |
| `MessageStream` 必须持续 poll，drop 时自动移除 match rule | 每条规则一个常驻任务；订阅失败降级为 Narration |
| 本库 `gtk` 模块名遮蔽 gtk4 crate 的 extern 名（E0463，`::gtk` 亦不可用） | Cargo 依赖重命名 `gtk_crate = { package = "gtk4" }` |
| SNI ARGB32 为网络字节序（字节序 [A,R,G,B]）、未预乘 | 对应 `gdk::MemoryFormat::A8r8g8b8`（非预乘变体），字节序天然对齐、无需换序 |
| `IconTheme` 逐 Display 取用且须 GTK 主线程 | `IconTheme::for_display` + 文档标注主线程约束；主题未命中 → `image-missing`；位图尺寸不完整 → 1×1 透明纹理（不 panic） |
| `call` 的 body 参数需 Sized（`&str` 会使 B=str） | GetAll 等字符串体临时 `to_string()`（一次性小分配） |

## 5. 测试设计

- **Watcher 名全总线唯一** → 生命周期测试必须串行：全部阶段收拢进单个
  `#[test]`，避免并行抢名。
- **mock 架构**：mock Item / mock dbusmenu 以独立的 CurrentThread 运行时线程
  驱动（`handle.spawn` + std 通道桥，与库内同步桥同构），与被测库同进程同总线。
- **总线隔离**：`dbus-run-session -- cargo test` 全量运行；真实桌面已有他人
  Watcher 时生命周期用例**跳过**（不误报）；单元测试均为纯函数，任意环境可跑。
- **真实总线容忍**：真实会话上快照可能含其他真实 Item——断言一律按 id 过滤，
  不假设快照为空；fire-and-forget 回执以轮询等待（`wait_until`/`recv_matching`）。
- **双注册并入**：同一对象重复注册按字段匹配两轮 lookup（注册名 ↔ service、
  发送者唯一名 ↔ unique）命中原地更新（CODE §3.3.1）——断言同一对象恒为
  单一 id；临时唯一名 → 知名名升级断言 NameChanged{old, new}，
  Changed/MenuChanged 落点确定。

## 6. 已知限制与后续方向
- **总线守护重启的 Displaced**：依赖 NOC/心跳检测，未在实测中复现（需杀
  dbus-daemon）；停机路径对名释放已做 NOC 区分。
- **发现深度**：反向发现只探测 `/StatusNotifierItem`（规范默认路径）；
  对象路径非默认且未主动报到的 Item 无法发现（规范未提供路径查询手段）。
