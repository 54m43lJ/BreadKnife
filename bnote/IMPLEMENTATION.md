# bnote 实现备注

> 本文记录 bnote 开发期间发现的文档未覆盖细节、文档与 zbus/规范不一致之处，以及实现采用的边界行为。
> 协议权威文本仍然是 `bnote/reference/`；宿主契约、实现骨架与依赖契约分别见 `USER.html`、`CORE.html`、`PROVIDER.html`。
>
> 最后更新：2026-10-20

## 1. 文档缺口与不一致

### 1.1 `hints::parse` 缺少 `app_icon` 入参

`CORE §4.7` 给出：

```rust
pub(crate) fn parse(hints: &HashMap<String, zbus::Value>) -> HintSet;
```

但 `PROVIDER §4.3` 的图标优先级是 `image-data > image-path > app_icon > icon_data`，其中 `app_icon` 是 `Notify` 方法参数，不在 hints 字典里。按 `CORE` 的签名无法完成归一。

实现改为：

```rust
pub(crate) fn parse(hints: &HashMap<String, OwnedValue>, app_icon: &str) -> HintSet;
```

建议回填 `CORE §4.7`，并在 `HintSet.icon` 的说明中明确它包含 `app_icon` 与 `icon_data` 的回退结果。

### 1.2 启动顺序：先注册对象，再申请总线名

`CORE §5.1` 的顺序是：

```text
request_name → object_server().at(...)
```

实现采用相反顺序：

```text
Connection::session()
→ object_server().at("/org/freedesktop/Notifications", Server)
→ request_name_with_flags("org.freedesktop.Notifications", DoNotQueue)
```

原因：名称一旦可见，客户端就可能立即调用方法；此时接口必须已经注册，否则调用会丢失。zbus 的 `request_name_with_flags` 文档也明确提示应先装配对象服务器。

建议把 `CORE §5.1` 调整为“注册对象 → 申请名字”。

### 1.3 `Server` 不能持有 `Bus`

`CORE §4.5` 把 `Server` 描述为“持有 `Bus` 句柄”。按字面实现会形成引用环：

```text
Bus → zbus::Connection → ObjectServer → Server → Bus
```

连接因此无法在会话停机时释放，总线名也会一直被占用。

实现中 `Server` 只持有：

```rust
pub(crate) struct Server {
    caps: Capabilities,
    info: ServerInfo,
    events: Arc<EventQueue>,
    state: Arc<Mutex<State>>,
    stopped: Arc<AtomicBool>,
}
```

`CloseNotification` 需要发信号时，通过 zbus 的 `#[zbus(connection)]` 注入当前 `&Connection`，不保存连接。建议更新 `CORE §4.5`。

### 1.4 `RuntimeShared` 的 `Bus` 必须可丢弃

`CORE §4.2` 给出 `RuntimeShared { bus: Bus, join: ... }`。如果显式 `shutdown` 时只是把线程 join 掉，`Bus` 仍被 `RuntimeShared` 持有，连接、对象接口和总线名都不会释放。

实现把 `bus` 存成：

```rust
bus: Mutex<Option<Bus>>
```

`shutdown` 的顺序是：

1. 原子停机标记并唤醒内部 runtime；
2. 关闭事件队列；
3. join 内部线程；
4. 从 `Mutex<Option<Bus>>` 取出并 drop `Bus`，释放连接与总线名。

停机不经过 `Bus::cmd(stop)`：直接设置 stop watch 可以避免命令通道在停机时阻塞，也能覆盖 runtime 正忙或命令已超时的情况。

建议在 `CORE §4.2`、`CORE §5.7` 中补充“停机后必须释放连接，而不只是停止 runtime”。

### 1.5 `Bus::spawn_runtime` 的实际签名

`CORE §4.3` 给出：

```rust
pub(crate) fn spawn_runtime(config: SessionConfig)
    -> (mpsc::Receiver<Result<Bus, Error>>, JoinHandle<()>);
```

实现需要接收由 `Session::start` 创建的事件队列，并且线程创建本身也可能失败：

```rust
pub(crate) fn spawn_runtime(
    config: SessionConfig,
    events: Arc<EventQueue>,
) -> Result<(StartupReceiver, JoinHandle<()>), Error>;
```

事件队列也可以由 `spawn_runtime` 内部创建，但 `EventQueue::new` 还需要把 `NoticeReceiver` 交回宿主，拆成外部创建更直接。建议更新 `CORE §4.3`。

### 1.6 `EventQueue::send` 需要报告投递失败

`CORE §4.4` 的 `send` 返回 `()`。实现返回 `bool`：

```rust
pub(crate) fn send(&self, event: NoticeEvent) -> bool;
```

用途：

- `Notify` 的事件投递失败时，从在册集合移除刚分配的 id，并返回 D-Bus `Failed`；
- `CloseNotification` 的事件投递失败不改变客户端结果：信号发射成功则客户端成功，发射失败则返回 D-Bus `Failed`；
- 会话停机后所有发送都返回 `false`。

建议在 `CORE §4.4` 中说明事件通道关闭后的行为。

### 1.7 `Server::notify` 不能直接接收 `NotifyParams`

`CORE §4.5` 写成：

```rust
pub(crate) async fn notify(&self, params: NotifyParams) -> Result<u32, Error>;
```

zbus 接口方法必须逐个声明 D-Bus 参数，不能把结构体自动展开成参数列表。实现对外的接口方法仍是逐个参数，内部再构造 `NotifyParams` 并调用 `notify_impl`：

```rust
async fn notify(
    &self,
    app_name: String,
    replaces_id: u32,
    app_icon: String,
    summary: String,
    body: String,
    actions: Vec<String>,
    hints: HashMap<String, OwnedValue>,
    expire_timeout: i32,
) -> zbus::fdo::Result<u32>;
```

建议在 `CORE §4.5` 中区分“D-Bus 接口参数”和“内部 `NotifyParams`”。

### 1.8 信号发射函数的 `Bus` 参数

`CORE §4.6` 的三个函数都接收 `&Bus`。由于 `Server` 不持有 `Bus`（见 §1.3），服务端方法无法直接调用这些函数。

实现保留宿主侧使用的 `emit_*` 函数，同时增加：

```rust
pub(crate) async fn emit_closed_on(
    conn: &Connection,
    id: u32,
    reason: CloseReason,
) -> Result<(), Error>;
```

`Server::close_notification` 使用注入的连接调用 `_on` 版本。建议在 `CORE §4.6` 中补充这一点。

### 1.9 `zbus::Value` 与 `OwnedValue`

`CORE §4.5` 和 `PROVIDER §4.1` 的草案使用 `HashMap<String, zbus::Value>`。`Value` 带借用生命周期，不适合作为 zbus 对象服务器方法的参数类型；实现使用：

```rust
HashMap<String, zbus::zvariant::OwnedValue>
```

D-Bus 签名仍然是 `a{sv}`，宿主 API 不受影响。建议统一文档中的类型写法。

### 1.10 公共 re-export 列表不完整

`CORE §4.1` 的 `lib.rs` 只列出：

```rust
Capabilities, CloseReason, Notice, NoticeEvent, ServerInfo, SessionConfig
```

但 `USER §4.3` 还定义了 `Action`、`Urgency`、`NoticeIcon`，它们是 `Notice` 的公开字段类型。实现额外 re-export：

```rust
pub use types::{Action, NoticeIcon, Urgency};
```

建议补齐 `CORE §4.1`。

### 1.11 `Error::Protocol` 作为兜底保留

原 `USER §4.2` 把 `Error::Protocol` 描述为“协议字段缺失/类型不符；宿主记录并跳过该通知”，但当前宿主 API 没有接收该错误的路径：

- D-Bus 参数类型不符由 zbus 在进入接口方法之前转成 `org.freedesktop.DBus.Error.InvalidArgs`，客户端收到错误；
- hints 字段类型不符按规范静默回退（`urgency` → `Normal`，非法图像 → 下一个图标来源）；
- `NoticeReceiver::recv` 只返回 `NoticeEvent`，没有错误事件。

最终处理：保留 `Error::Protocol` 作为兜底变体，Rustdoc 明确“当前正常路径不会产生，仅保留给未来例外情况”。`Server` 不再产生该错误（服务端错误已拆到 `ServerError`）；命令桥内部的 downcast 防御分支仍映射到它，属于不可达兜底。`USER §4.2` 已同步为兜底语义。

### 1.12 未知 id 的 `CloseNotification` 错误名

规范 §9.1.3 只说“通知不存在时返回一个空的 D-Bus Error”，没有规定错误名。`PROVIDER §4.4` 将 “id 非法” 归到 `InvalidArgs`。

实现返回：

```text
org.freedesktop.DBus.Error.InvalidArgs
消息体为空
```

这样既满足“空错误消息”，又与 `PROVIDER` 的错误分类一致。建议在 `PROVIDER §4.4` 明确错误名和空消息体。

### 1.13 `image-data` 的 `has_alpha == false` 且 `channels == 4`

规范 §5.2 写的是：

> If has_alpha is TRUE, must be 4, otherwise 3.

`PROVIDER §4.3` 则写成“8-bit、3/4 通道”，并只明确要求 `has_alpha` 为真时通道数为 4。

最终实现采用严格解释：

- `channels` 必须等于 `if has_alpha { 4 } else { 3 }`；
- `has_alpha == false` 且 `channels == 4` 视为非法，回退到下一个图标来源；
- `bits_per_sample` 仍必须为 8，尺寸/rowstride/数据长度校验保持不变。

调研结论：KDE Plasma 接受 4 通道并忽略 `has_alpha` 一致性检查（把第 4 通道当 alpha），GNOME Shell / dunst 直接按 `has_alpha` 解释而忽略 channels 字段，Quickshell / denialwm 则严格校验 `channels == (has_alpha ? 4 : 3)`。没有形成“4 通道 + 无 alpha 当作不透明图”的业界共识，因此按规范严格解释。`PROVIDER §4.3` 已同步。

### 1.14 `command_timeout_ms` 不覆盖 `Session::start`

`USER §4.2` 的 `Error::Timeout` 描述为“同步命令在 `command_timeout_ms` 内未完成”，`Session::start` 也是同步调用，但内部没有用 `command_timeout_ms` 限制启动。

当前行为：

- `SessionHandle` 的 `close` / `invoke_action` / `activation_token` 受 `command_timeout_ms` 约束；
- `Session::start` 等待内部线程的 ready 结果，直到连接成功、NameTaken 或连接失败；
- `shutdown` 直接 join 内部线程，不受超时约束。

已按“超时由宿主管理”处理：`command_timeout_ms` 只约束 `SessionHandle` 命令；`Session::start` 与 `shutdown` 的等待超时由宿主放到工作线程并自行设置。`USER §4.3` 已在 `SessionConfig.command_timeout_ms` 定义中写明这一点，`Error::Timeout` 的描述也已限定为 `SessionHandle` 同步命令。

### 1.15 关闭、替换与事件投递的并发时序

文档只给出单线程调用栈，没有说明以下并发情况：

- `Notify` 分配 id 后、把 `NoticeEvent::Notify` 放进队列前，`CloseNotification` 是否可以先删除 id 并发送 `Close` 事件；
- 客户端关闭一个 id 后、`NotificationClosed` 发出前，另一个 `Notify(replaces_id = 同一 id)` 是否可以让该 id 重新生效；
- 宿主关闭与客户端 `CloseNotification` 同时发生时，谁负责发信号。

实现用 `state: tokio::sync::Mutex<State>` 把 id 状态、事件投递和 `NotificationClosed` 发射串行化：

- `Notify` 在持锁期间分配/登记 id 并投递事件；
- `CloseNotification` 在持锁期间删除 id、发射 `NotificationClosed`、投递 `Close` 事件；
- `SessionHandle::close` 在持锁期间删除 id 并发射信号；
- 后到的同 id 替换或关闭会等待前一个操作完成。

建议在 `CORE §5` 或 `USER` 中说明“同 id 的 Notify 与 Close 按协议状态串行处理”。

### 1.16 宿主 `close` 的未知 id 与重复关闭

文档没有说明 `SessionHandle::close` 对不在册 id 的处理。实现选择：

- 先删除 id，再发 `NotificationClosed`；
- id 不在册时返回 `Error::InvalidArgs`；
- 客户端 `CloseNotification` 与宿主 `close` 竞争时，只有一方能删除 id，因此最多发一次信号。

上述行为已写入 `CORE §5.4`；`USER §4.1` 同步说明 id 在册校验。

### 1.17 `invoke_action` / `activation_token` 的 id 校验

最终决定：只校验 id 是否在册。

- `invoke_action` / `activation_token` 对未知或已失效 id 返回 `Error::InvalidArgs`；
- action key 是否属于原通知的 actions **暂不校验**：它需要库内缓存每个通知的 action key，会扩大状态面，留待未来考虑；
- 该行为已写入 `USER §4.1` 与 `CORE §5.5` / `§5.6`。

`State` 增加 `contains(id)`，校验与信号发射在同一把 state 锁内完成，避免校验后 id 立即被关闭。

### 1.18 事件接收端丢弃后的行为

`CORE §4.4` 使用 `mpsc::UnboundedSender`，但没有说明：

- 宿主丢弃 `NoticeReceiver` 后 `Notify` 是否还应成功；
- `CloseNotification` 是否仍应给客户端回信号。

最终决定：保留当前非对称行为，并忠实记录在 `CORE §5.2` / `§5.3`。

- `Notify` 的 id 返回是“通知已接收、可供宿主处理”的承诺；宿主事件投递失败则撤销 id 并返回 D-Bus `Failed`。
- `CloseNotification` 的关闭契约以 id 失效 + `NotificationClosed` 为准；宿主 `Close` 是辅助通知，投递失败不回滚关闭，也不改变 D-Bus 结果。
- 因此不存在“返回 Failed 后又发 Closed”的组合。
- 没有可援引的业界共识：主流 daemon 没有 bnote 这样的 host channel；这是 bnote 自己的边界。

`PROVIDER §4.4` 同步说明：未知 hints/urgency/image-data 不回错误；`CloseNotification` 的宿主事件失败不改变 D-Bus 结果。

### 1.19 Fatal 之后连接与总线名的释放

`USER §4.2` 说 Fatal 后宿主决定是否重建会话，但没有说明旧连接何时释放。当前实现：

- Fatal 路径先置停机标记，入队 Fatal 后关闭事件通道；
- 内部 runtime 退出；
- 但 `Bus` 仍由 `SessionHandle` 持有，连接和总线名要等宿主调用 `shutdown` 或丢弃最后一个 handle 才释放；
- 因此宿主重建会话前必须先停机/丢弃旧 handle，否则可能得到 `NameTaken`。

已写入 `USER §4.2` 的 Fatal 行：宿主重建会话前必须先 `shutdown` 或丢弃旧 handle。`CORE §5.8` 同步记录。

### 1.20 心跳参数

`CORE §5.8` 提到“心跳失败”，但没有给出周期和超时。实现使用：

- 每 30 秒调用一次 `org.freedesktop.DBus.GetId`；
- 单次调用超时 5 秒；
- 连接 `closed()` 事件也直接触发 Fatal。

已写入 `CORE §5.8`：说明 `closed()` + 心跳机制，并在调用栈注释中标注推荐值（30s 周期、5s 超时；实际值以代码为准）。

### 1.21 文件树中的示例

`CORE §1` 文件树已补 `examples/server.rs`，用于交互式测试；其余实现产物不写入接口文档。

### 1.22 宿主错误与 D-Bus 错误的域区分

已按“明确区分错误域”实现：

- 宿主侧只使用 `bnote::Error`；`InvalidArgs` 用于 `Session::start` 配置、未知/失效 id 等宿主参数错误；
- D-Bus 服务端使用独立的 `ServerError::{InvalidArgs, Failed}`，直接映射到 `org.freedesktop.DBus.Error.InvalidArgs` / `.Failed`；
- `Server` 不再使用 `bnote::Error`，因此不存在两边复用同一变体的问题；
- `PROVIDER §4.4` 明确线上只有 D-Bus 错误名，`USER §4.2` 明确宿主侧 `Error::InvalidArgs` 的含义；
- `Error::Protocol` 只保留为兜底（见 §1.11）。

## 2. 文档未覆盖的实现约定

### 2.1 启动与总线名

- 使用 `Connection::session()` 连接会话总线。
- 先注册对象路径 `/org/freedesktop/Notifications`，再申请服务名。
- `RequestNameFlags::DoNotQueue`：不替换已有守护、不允许被替换、被占用时不排队。
- `PrimaryOwner` / `AlreadyOwner` 视为成功；`InQueue` / `Exists` 以及 zbus 的 `NameTaken` 都归一为 `Error::NameTaken`。
- 能力清单在启动前校验：非空、仅 ASCII 字母数字与 `-`；`icon-static` 与 `icon-multi` 互斥；失败返回 `Error::InvalidArgs`。
- 启动失败会 join 内部线程；连接随 `Bus` 一起释放。

### 2.2 生命周期

- `Session::start` 创建独立线程和 current-thread Tokio runtime。
- `SessionHandle` 内部维护原子 handle 计数；最后一个实例 drop 时触发停机。
- `shutdown` 幂等，阻塞到内部 runtime 退出，并释放连接与总线名。
- `NoticeReceiver` 不持有会话生命周期；丢弃接收端不会停机，但会令后续 `Notify` 事件投递失败。
- `SessionHandle` 的 drop 也可能阻塞（join 内部线程），宿主应像对待 `shutdown` 一样避免在实时线程中丢弃最后一个 handle。
- 停机后重新调用 `Session::start` 已验证可以立即重新取得总线名。

### 2.3 命令桥

- `Bus::cmd` 把闭包封装成类型擦除的 future，投递到内部 runtime，宿主线程用 `std::sync::mpsc::recv_timeout` 等待结果。
- 超时返回 `Error::Timeout`；内部命令没有被取消，信号可能在超时之后仍然发出。
- runtime 已停机、命令通道关闭或 `Bus` 已被 `shutdown` 取走时返回 `Error::Stopped`。
- 命令在 runtime 上以独立 task 执行，彼此可以并发；协议状态由 `State` 锁串行化。
- 命令返回值通过 `Box<dyn Any + Send>` 回传；类型不匹配返回 `Error::Protocol`。

### 2.4 ID 状态与关闭

- `State.next_id` 从 1 开始，跳过 0 和当前在册 id；到 `u32::MAX` 后回绕到 1。
- `replaces_id != 0` 时直接沿用并登记，不要求原 id 之前存在。
- `State::contains(id)` 用于宿主命令的 id 在册校验。
- 客户端 `CloseNotification`：在册校验 → 删除 id → 发 `NotificationClosed(CloseRequest)` → 投递 `NoticeEvent::Close`。
- 宿主 `close`：删除 id → 发 `NotificationClosed(reason)`；id 不在册返回 `InvalidArgs`。
- 宿主 `invoke_action` / `activation_token`：校验 id 在册后在 state 锁内发信号；未知 id 返回 `InvalidArgs`。action key 归属暂不校验。
- 信号发射失败时，客户端关闭仍会向宿主投递 `Close` 事件；宿主关闭则返回 `Error::Bus`，但 id 已经失效。
- `ActionInvoked` / `ActivationToken` 不改变 id 状态。

### 2.5 `Notify` 归一

- actions 必须成对；奇数个在分配 id 之前返回 D-Bus `InvalidArgs`。没有过滤空 key/label。
- `expire_timeout`：负数 → `None`，0 → `Some(0)`，正数 → `Some(ms)`。
- urgency：只接受 BYTE（`u8`）0/1/2；缺失、未知值、类型不符 → `Normal`。
- 图标：
  - 优先级 `image-data > image-path > app_icon > icon_data`；
  - 别名支持 `image_data`、`image_path`、`icon_data`、`icon-data`；
  - `app_icon` / `image-path` 为空时继续回退；
  - `image-data` / `icon_data` 至少要有 7 个字段 `(iiibiiay)`；多余字段忽略；
  - `width`、`height`、`rowstride` 必须为正；
  - `bits_per_sample` 必须为 8，`channels` 必须等于 `if has_alpha { 4 } else { 3 }`；不满足则回退；
  - `rowstride >= width * channels`，数据至少覆盖最后一行的像素；最后一行可省略尾部 padding；
  - 输出按 `width * height * 4` 紧密排列为 RGBA8；3 通道时 alpha 填 255；
  - 解析失败不报错，继续尝试下一个图标来源；全部失败为 `NoticeIcon::None`；
  - 尺寸乘法使用 checked 运算，容量用 `try_reserve_exact`，异常输入直接回退。
- 未知 hint 键忽略。
- `Notify` 事件投递失败时撤销刚分配的 id 并返回 D-Bus `Failed`。

### 2.6 D-Bus 错误与信号

- D-Bus 服务端使用独立的 `ServerError`：`InvalidArgs` → `org.freedesktop.DBus.Error.InvalidArgs`，`Failed` → `org.freedesktop.DBus.Error.Failed`。
- 宿主侧 `bnote::Error` 只在 `Session` / `SessionHandle` 层使用；服务端不再复用 `bnote::Error`。
- 未知 id 的 `CloseNotification` 用 `InvalidArgs` + 空消息体。
- 三个信号都在 `#[zbus::interface]` 中声明，以便 introspection 可见；实际发射由 `signal.rs` 通过 `Connection::emit_signal` 完成。
- `NotificationClosed` 的 reason 由 `CloseReason::code()` 固定为 1/2/3；不使用 4。
- 信号广播到总线上所有客户端（`destination = None`）。

### 2.7 Fatal 与停机

- 内部监视任务同时等待连接 `closed()` 和 30 秒心跳。
- 失败时先原子置停机标记并唤醒 runtime（current-thread runtime 不会在 Fatal 入队前抢占执行），随后入队 `NoticeEvent::Fatal`、关闭事件队列。
- 显式 `shutdown` 不发送 Fatal：置停机标记、关闭事件通道并退出 runtime。
- Fatal 后 `SessionHandle` 命令返回 `Stopped`，但连接要等 `shutdown` 或最后一个 handle 被丢弃才释放。

## 3. 验证

### 3.1 命令

```bash
cd bnote
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets

dbus-run-session -- env BNOTE_TEST_DBUS=1 cargo test --all-targets
```

`cargo test` 的单元测试不接触会话总线；集成测试只在 `BNOTE_TEST_DBUS=1` 且存在 `DBUS_SESSION_BUS_ADDRESS` 时运行，避免误连开发桌面上的真实通知守护。

### 3.2 覆盖范围

已覆盖：

- 能力与服务器信息查询；
- Notify 新 id、replaces_id 替换、actions、urgency、expire_timeout；
- image-data 归一（RGBA、rowstride、alpha）；
- 宿主 close / action / activation token 三类信号；
- 客户端 CloseNotification 与 reason 3；
- 未知 id / 重复关闭的 InvalidArgs；
- 宿主 invoke_action / activation_token 对未知/已失效 id 的 InvalidArgs；
- actions 奇数个的 D-Bus InvalidArgs；
- 第二个 Session 的 NameTaken；
- 最后一个 handle drop 自动停机、停机后重建会话。

开发过程中还使用 `dbus-run-session` 配合 `notify-send`、`gdbus` 做了外部客户端验证：

```bash
dbus-run-session -- bash -c '
  <临时宿主程序> &
  sleep 1
  gdbus call --session -d org.freedesktop.Notifications \
    -o /org/freedesktop/Notifications \
    -m org.freedesktop.Notifications.GetCapabilities
  notify-send -u critical -t 5000 "Hello" "Body"
  gdbus introspect --session -d org.freedesktop.Notifications \
    -o /org/freedesktop/Notifications
  wait
'
```

验证了 `notify-send` 的 critical urgency / timeout 归一、`GetServerInformation` 应答，以及 introspection 中三个信号的完整签名。

### 3.3 示例服务器

```bash
dbus-run-session -- cargo run --example server -- \
  --name bnote-server --vendor breadknife \
  --caps actions,body,body-markup,icon-static
```

示例采用同步宿主结构：主线程直接调用 `SessionHandle` 的阻塞命令，`NoticeReceiver` 放在独立线程的 current-thread runtime 中消费。stdin 支持 `list` / `close` / `action` / `token` / `info` / `quit`；总线名被占用时打印提示并以失败退出，不排队、不替换已有守护。

`cargo test --all-targets` 会一并编译 `examples/server.rs`，防止示例停留在旧 API。

## 4. POC 文档合并冲突

> 合并来源：`poc/notification`。规则：HTML 文档冲突直接取 poc 版本；与源码 / `IMPLEMENTATION` 的冲突只记录，不修改源码，等待具体修改意见。
> 合并后旧 `ARCHITECTURE/CODE/BUILD` 与新 `USER/CORE/PROVIDER` 并存，旧单文件 reference 与新分章 reference 并存。

### 4.1 HTML 文档冲突（已直接取 poc）

| 文件 | 冲突类型 | 处理 |
|---|---|---|
| `CODE.html`（根） | content conflict | 取 poc 版本 |
| `bnote/README.html` | content conflict | 取 poc 版本 |
| `bnote/ARCHITECTURE.html` | modify/delete | 恢复 poc 版本 |
| `bnote/CODE.html` | modify/delete | 恢复 poc 版本 |
| `bnote/BUILD.html` | poc 新增 | 采用 poc 版本 |
| `ARCHITECTURE.html`、`BUILD.html`、`README.html`、`style.css`（根） | poc 修改 | 已应用 poc 修改 |
| `bnote/reference/notification-spec.clean.html` | dev 已删除，poc 旧文档仍引用 | 从合并基点恢复；分章 reference 同时保留 |

结果：旧文档与新文档同时存在；`bnote/README.html` 现在指向旧 `ARCHITECTURE/CODE/BUILD`，新 `USER/CORE/PROVIDER` 仍保留但暂时没有 README 入口。这是 HTML 层面的结构重复，已按“冲突优先 poc”直接合并，未擅自删除新文档。

### 4.2 与源码 / IMPLEMENTATION 的冲突（待意见）

| ID | poc 文档描述 | 当前源码 / IMPLEMENTATION | 涉及文件 |
|---|---|---|---|
| C1 | 异步 `Session::start(&Connection, Sender<NoticeEvent>, Capabilities, ServerInfo) -> Rc<Session>` | 同步 `Session::start(SessionConfig) -> (SessionHandle, NoticeReceiver)` | `bnote/ARCHITECTURE.html`、`bnote/CODE.html`、`bnote/BUILD.html`、`bnote/README.html` |
| C2 | 名字被占用时进入 `SessionState::Queued`，`state()` / `subscribe()` 观察，之后自动接管 | 被占用时立即返回 `Error::NameTaken`，不排队、不替换；宿主重新调用 `Session::start` | 旧 bnote 文档、根 `README/ARCHITECTURE/CODE/BUILD` |
| C3 | `Rc<Session>` + 自由函数 `bnote::close/invoke_action/activation_token`；有界事件通道 | `SessionHandle` 命令 + `NoticeReceiver`；无界事件队列 | `bnote/CODE.html`、`bnote/ARCHITECTURE.html`、`bnote/BUILD.html` |
| C4 | 未规定 `NoticeReceiver` 丢弃后的行为 | 当前实现准备按第二部分 2.2 改为“丢弃即终止整个会话”；旧文档没有这一条 | 旧 bnote 文档、根通知中心文档 |
| C5 | 旧文档没有命令超时 / 取消语义 | 当前 `command_timeout_ms` + 每命令独立响应通道；第二部分 2.3 将补充取消与迟到响应清理 | `bnote/ARCHITECTURE.html`、`bnote/CODE.html`、`bnote/BUILD.html` |
| C6 | 旧 `CloseReason` 文档只列 1/2/3 | 规范还定义 4（undefined/reserved）；实际不可达，需在文档中保留并注解 | 旧 bnote 文档、`bnote/PROVIDER.html`、`bnote/USER.html` |
| C7 | poc `bnote/IMPLEMENTATION.md` 描述旧异步 API、排队接管、旧错误类型 | 保留当前 `IMPLEMENTATION.md`（sync API、NameTaken、ServerError 等）；poc 文件内容未合并 | `bnote/IMPLEMENTATION.md` |
| C8 | poc `bnote/BUILD.html` 使用 `enumflags2`、`futures-util`、`tokio sync` 与异步示例 | 当前 Cargo 仅运行期依赖 tokio（macros/rt/sync/time）与 zbus；示例是同步宿主 + 事件线程 | `bnote/BUILD.html`、`bnote/Cargo.toml`、`bnote/examples/server.rs` |
| C9 | 根文档描述通知中心排队不可用态、等待接管提示 | 当前 bnote 返回 `NameTaken`，不存在排队态；第二部分 2.1 将改为 `NameTaken` 报错 + 手动刷新重试 | 根 `README.html`、`ARCHITECTURE.html`、`CODE.html`、`BUILD.html` |
| C10 | 旧文档与新 `USER/CORE/PROVIDER` 对同一 API / 行为给出不同描述 | 新文档是当前实现的第一来源；旧文档保留待统一 | `bnote/` 下两套文档 |

源码侧合并冲突（`bnote/Cargo.toml`、`bnote/src/*`、`bnote/examples/server.rs`）均保留当前 dev 版本；poc 代码没有并入。

## 5. 回填结果

| 细节 | 落点 | 状态 |
|---|---|---|
| `hints::parse` 增加 `app_icon` 参数 | CORE §4.7 | 已回填 |
| 启动顺序改为“注册对象 → 申请名字” | CORE §5.1 | 已回填 |
| `Server` 不持有 `Bus`，使用注入连接发信号 | CORE §4.5、§4.6 | 已回填 |
| `RuntimeShared` 用 `Option<Bus>` 并在停机时释放连接 | CORE §4.2、§5.7 | 已回填 |
| `spawn_runtime` 接收事件队列并返回 `Result` | CORE §4.3 | 已回填 |
| `EventQueue::send` 返回投递结果 | CORE §4.4 | 已回填 |
| `NotifyParams` 是内部类型，接口方法逐个收参数 | CORE §4.5 | 已回填 |
| 文档类型从 `zbus::Value` 改为 `OwnedValue` | CORE §4.5、PROVIDER §4.1 | 已回填 |
| 公共 re-export 补齐 `Action` / `Urgency` / `NoticeIcon` | CORE §4.1 | 已回填 |
| `Error::Protocol` 保留为兜底，正常路径不产生 | USER §4.2 | 已定 |
| 明确未知 id 的错误名与空消息体 | PROVIDER §4.4 | 已回填 |
| `has_alpha=false` 且 4 通道按严格解释拒绝 | PROVIDER §4.3 | 已回填 |
| 明确 `command_timeout_ms` 只作用于 `SessionHandle` | USER §4.1、§4.3 | 已回填 |
| 补充同 id Notify/Close 的串行语义 | CORE §5 / USER §4 | 已回填 |
| 补充宿主 `close` 未知 id 的行为 | CORE §5.4、USER §4.1 | 已回填 |
| 补充 action/token 的 id 校验；key 归属暂不实现 | USER §4.1、CORE §5.5 / §5.6 | 已回填 |
| 补充接收端丢弃与事件投递失败行为 | CORE §4.4 / §5.2 / §5.3、PROVIDER §4.4 | 已回填 |
| 补充 Fatal 后旧连接/总线名的释放要求 | USER §4.2、CORE §5.8 | 已回填 |
| 补充心跳机制与推荐取值 | CORE §5.8 | 已回填 |
| 文件树补 `examples/server.rs` | CORE §1 | 已回填 |
| 区分宿主错误与 D-Bus 错误 | USER §4.2、PROVIDER §4.4、CORE §4.5 | 已回填 |
