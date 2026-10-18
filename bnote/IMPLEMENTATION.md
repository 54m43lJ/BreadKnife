# bnote 实现备注

> 本文件记录实现期间遇到的、设计文档未写明或只给出签名的细节。协议行为以
> `reference/notification-spec.clean.html` 为权威；本文只记录 bnote 采用的边界行为、
> 实现约束与建议回填到 ARCHITECTURE / CODE / BUILD 的内容。
>
> 最后更新：2026-10-18

## 1. 会话装配与生命周期

### 1.1 `Session::start` 必须是异步的

设计稿写的是同步签名；实现改为：

```rust
pub async fn start(
    conn: &zbus::Connection,
    tx: Sender<NoticeEvent>,
    caps: Capabilities,
    info: ServerInfo,
) -> Result<Rc<Self>, bnote::Error>
```

原因是 zbus 的对象注册与总线名申请都是异步操作：

- `conn.object_server().at(PATH, server).await`
- `conn.request_name(BUS_NAME).await`

宿主必须 `.await`。不能在没有运行时的上下文里同步调用。

### 1.2 注册顺序与失败回滚

`start` 的顺序固定为：

1. 校验能力清单（见 §4.6）；
2. 注册 `/org/freedesktop/Notifications` 接口；
3. 申请 `org.freedesktop.Notifications` 总线名。

先注册对象、再申请名字，避免名字生效后调用到达时接口尚未就位。`request_name` 失败时会把刚注册的接口移除，避免留下半装配状态。

### 1.3 不抢占已有通知守护

`request_name` 使用 zbus 默认标志，即 `DoNotQueue`，不请求替换现有所有者。桌面已有 dunst / mako / 桌面环境通知守护时，`start` 返回 `Error::Bus`。这是预期行为，不是库内自动降级。

### 1.4 一个连接只能装配一个会话

同一连接上重复 `Session::start` 时，`object_server().at()` 返回 `false`，库返回 `Error::InvalidArgs`。不同连接之间仍由总线名所有权决定谁成功。

### 1.5 没有 `stop` / `Drop` 清理

`Session` 只持有 `zbus::Connection`；接口和总线名归连接所有。当前没有：

- `Session::stop()`；
- `Drop` 中注销接口或释放总线名。

因此，只要连接还活着，服务就继续存在；宿主需要停服时应释放连接或自行调用 zbus 的 `ObjectServer::remove` / `Connection::release_name`。这是当前实现的已知缺口，不应被理解为“会话对象销毁即停服”。

### 1.6 事件通道背压

库接收 `tokio::sync::mpsc::Sender<NoticeEvent>`。`notify` 与 `close_notification` 都会 `await` 发送事件：

- 宿主必须持续消费事件，否则有界通道满时 D-Bus 方法任务会被阻塞；
- 接收端关闭时发送失败被忽略，`Notify` 仍会返回 id，`CloseNotification` 仍会继续回发关闭信号。

示例使用容量 16 的有界通道。

### 1.7 `close()` 依赖已注册接口

宿主回发关闭的 `close(conn, id, reason)` 通过 `ObjectServer` 查找 `server::Server` 接口来更新协议层在册集合。如果接口不存在，或 id 已不在册，函数直接返回 `Ok(())`，不再发射信号。这样：

- 客户端关闭与宿主关闭竞争时不会重复发 `NotificationClosed`；
- 但库重启后，宿主若还保留旧 id 并调用 `close()`，不会为该 id 发信号。

## 2. D-Bus 方法与错误

### 2.1 hints 参数用 `OwnedValue`

`Notify` 的 `a{sv}` 在 zbus 接口方法中落地为：

```rust
HashMap<String, zbus::zvariant::OwnedValue>
```

实现采用 `OwnedValue`：它避免接口方法参数的生命周期约束，也是 zbus 对象服务器反序列化 `a{sv}` 后的自然形态。

### 2.2 服务端错误类型与公开错误类型分离

公开 API 的 `bnote::Error` 面向宿主调用方，包含 `Bus` 与 `InvalidArgs`。D-Bus 方法内部返回 `zbus::fdo::Result<()>`，这样 zbus 才能把错误编码成标准 D-Bus Error。两者不混用。

### 2.3 未知 id 的 `CloseNotification`

规范 §9.1.3 要求通知不存在时返回空 D-Bus Error。实现返回：

```rust
zbus::fdo::Error::Failed(String::new())
```

实际错误名为 `org.freedesktop.DBus.Error.Failed`，消息体为空。已验证重复关闭第二次调用会得到该错误。

### 2.4 方法返回值的 out 参数名

`GetServerInformation` 的四个返回值通过：

```rust
#[zbus(out_args("name", "vendor", "version", "spec_version"))]
```

命名；`Notify` 的单个返回值通过 `#[zbus(out_args("id"))]` 命名。它们只影响内省数据，不影响调用方按位置取值。

### 2.5 id 分配规则

协议层状态只保存 `next_id` 与在册 id 集合：

- 新 id 从 1 开始；
- `u32::MAX` 后回到 1；
- 跳过 0；
- 跳过当前在册 id，避免复用尚未关闭的通知 id。

`replaces_id != 0` 时直接沿用该值并登记为在册，不检查它原先是否存在。规范要求返回同一个值，因此这是允许的。

### 2.6 `CloseNotification` 的时序

`Server::close_notification` 的顺序是：

1. 从在册集合移除 id；不存在则返回 D-Bus Error；
2. 发送 `NoticeEvent::Close { id }` 给宿主；
3. 发射 `NotificationClosed(id, 3)`。

如果第 3 步发射失败，方法返回 D-Bus Error，但 id 已移除、`Close` 事件已投递。宿主仍需移除界面；这一边界没有额外的补偿信号。

### 2.7 宿主关闭的去重

宿主调用 `close()` 时：

- `mark_closed(id)` 返回该 id 是否还在册；
- 只有在册时才发 `NotificationClosed`。

因此客户端 `CloseNotification` 与宿主超时/用户关闭同时发生时，最多只有一个信号。

## 3. 信号

### 3.1 信号声明与发射分离

接口内用 `#[zbus(signal)]` 声明三个信号，目的是让内省数据包含信号；实际发射由 `signal.rs` 通过：

```rust
conn.emit_signal(None, PATH, INTERFACE, "NotificationClosed", &(id, reason))
```

完成。这样既保留 ARCHITECTURE §4.1 的自由函数 API，又能让 `gdbus introspect` 看到信号。信号名、对象路径、接口名集中定义在 `lib.rs` 常量中。

### 3.2 reason 编码

`CloseReason::code()` 固定映射：

| 枚举 | reason |
|---|---|
| `Expired` | 1 |
| `Dismissed` | 2 |
| `CloseRequest` | 3 |

### 3.3 动作与令牌不校验在册 id

`invoke_action` / `activation_token` 只负责发射信号，不检查 id 是否还在册，也不检查 `action_key` 是否属于该通知。是否允许回发、回发后是否关闭通知，全部由宿主决定。

### 3.4 动作不会自动关闭通知

规范没有要求 `ActionInvoked` 后自动关闭。bnote 也不自动关闭；宿主可自行决定是否再调用 `close()`。

## 4. hints 与数据归一

### 4.1 超时映射

`Notify` 的 `expire_timeout` 按规范映射为：

| 入参 | `expire_timeout_ms` | 含义 |
|---|---|---|
| `-1`（或任意负数） | `None` | 宿主决定 |
| `0` | `Some(0)` | 常驻，不自动过期 |
| `> 0` | `Some(ms)` | 毫秒 |

设计稿曾把 `None` 写成“常驻”、`0` 写成“宿主默认”，与规范相反；实现以规范为准。

### 4.2 urgency 的容错

规范定义 hint `urgency` 为 byte：0 / 1 / 2。实现额外接受 `u32` / `i32` / `i64` 形态，并做范围转换；无法识别或超出 0–2 的值回退为 `Normal`。

### 4.3 actions 是参数，不是 hint

`actions` 来自 `Notify` 的 `as` 参数，不是 `a{sv}` hints。解析规则：

- 按顺序每两个字符串为一组（key, label）；
- 末尾落单的 key 被忽略；
- 没有对空 key 或空 label 做额外过滤。

### 4.4 图标优先级与别名

优先级从高到低：

1. `image-data` / `image_data` / `icon_data` / `icon-data` 位图；
2. `image-path` / `image_path` 路径；
3. `Notify` 的 `app_icon` 参数。

任一位图解析失败会继续尝试下一个别名，再回退到路径与 `app_icon`。空 `app_icon` 归一为 `NoticeIcon::None`。

### 4.5 `image-data` 归一为 RGBA8

规范签名是 `(iiibiiay)`。实现只接受：

- `bits_per_sample == 8`；
- `channels == 3` 或 `4`；
- `rowstride >= width * channels`；
- 数据至少覆盖到最后一行像素末尾；
- 最后一行可以不包含尾部 padding。

输出统一为紧密排列的 `width * height * 4` 字节 RGBA：

- 3 通道补 alpha 255；
- 4 通道且 `has_alpha == true` 时使用原 alpha；
- 4 通道但 `has_alpha == false` 时忽略 alpha，补 255。

16 位样本、多帧/动画、非法尺寸都会导致该 hint 被拒绝并回退到后续图标来源。

### 4.6 能力清单校验

`Session::start` 会校验：

- 能力名非空，且只含 ASCII 字母、数字与 `-`；
- `icon-static` 与 `icon-multi` 不能同时出现（规范明确互斥）。

不合法时返回 `Error::InvalidArgs`，且不会注册任何 D-Bus 对象。

## 5. 协议能力边界

以下行为是当前实现有意不做的，文档中应明确，避免宿主误以为库会代劳：

- **不解析 body markup**：`body` 原样交给宿主；即使声明了 `body-markup`，库也不剥离或校验标签。
- **不强制 `actions` 能力**：即使能力清单没有 `actions`，库仍会解析并投递动作；是否展示由宿主决定。
- **不处理其他 hints**：`category`、`desktop-entry`、`resident`、`transient`、`sound-file`、`sound-name`、`suppress-sound`、`x`、`y` 等均不进入 `Notice`。
- **不加载图标**：`NoticeIcon::Name` 保存主题名、文件路径或 URI 原文；库不查主题、不读文件、不解析 `file://`。宿主必须自行区分并加载。
- **不管理生命周期**：没有计时器、通知缓存、超时、持久化或通知中心逻辑；`expire_timeout_ms` 只是建议值。
- **不提供 D-Bus 激活服务文件**：bnote 是库，不安装 `org.freedesktop.Notifications.service`；宿主必须自己先启动并完成 `Session::start`。
- **不校验动作回发**：不检查 `ActionInvoked` 的 key 是否存在于原通知，也不检查通知是否仍在册。
- **不支持 16 位图像与多帧动画**：`image-data` 只取单帧 8 位数据。
- **不实现 `icon-multi`**：能力可以声明，但库没有动画帧模型；如果需要动画，宿主需要扩展类型。
- **不支持多会话**：一个连接只允许一个通知服务器会话。

## 6. 示例与测试备注

### 6.1 示例的异步装配

示例必须使用：

```rust
let _session = Session::start(&conn, etx, caps, info).await?;
```

`Session` 返回 `Rc`，示例把它绑定在 `main` 的局部作用域内，保证连接与接口在事件循环期间存活。

### 6.2 stdin 不能直接塞进 `tokio::select!`

设计稿示例使用 `std::sync::mpsc::Receiver::recv()`；这是阻塞调用，不能作为 `tokio::select!` 分支的 future。实现改为：

- 独立线程逐行读 stdin；
- 通过 `tokio::sync::mpsc::unbounded_channel` 送入异步事件循环。

### 6.3 外部客户端参数

- `dunstify -A` 的格式是 `"动作键,标签"`，不是 `"标签:键"`；
- `notify-send -t 0` 表示常驻；
- `notify-send -t -1` 表示交给宿主决定；
- `notify-send -r <id>` 对应 `replaces_id`。

### 6.4 内省信号需要显式声明

只在运行时用 `emit_signal` 发射、不在 `#[zbus::interface]` 中声明，`gdbus introspect` 的 signals 区会是空的。实现因此同时保留 `#[zbus(signal)]` 声明与手动发射。

### 6.5 已验证的手工测试矩阵

在 `dbus-run-session` 隔离总线中执行：

- `GetCapabilities` / `GetServerInformation`；
- `notify-send -u critical -t 5000`；
- `notify-send -r <id>` 替换；
- `dunstify -A "default,Open"` + 示例 `action` 命令；
- 示例 `token` 命令；
- 示例 `close <id> expired` 与客户端 `CloseNotification`；
- 重复 `CloseNotification` 返回空消息 D-Bus Error；
- `gdbus` 传入 `image-data` 并观察 RGBA 归一结果。

## 7. 建议回填文档

| 细节 | 建议落点 | 状态 |
|---|---|---|
| `Session::start` 异步、注册顺序、失败回滚 | ARCHITECTURE §4.1、CODE §3.1 | 已回填主要签名 |
| 无 `stop` / `Drop` 清理 | ARCHITECTURE §4.1 | 待补 |
| 事件通道背压与接收端关闭行为 | ARCHITECTURE §4.1 / CODE §4.1 | 待补 |
| 未知 id 的错误名与空消息 | CODE §3.2 | 待补 |
| id 分配从 1 开始、回绕、跳过 0 与在册 id | CODE §3.2 | 待补 |
| 关闭去重与 `close()` 的 no-op 边界 | ARCHITECTURE §4.2、CODE §4.2 | 已回填去重结论，边界待补 |
| 信号声明用于内省、实际由 `signal.rs` 发射 | CODE §3.3 | 待补 |
| `OwnedValue`、hints 别名、RGBA 归一细节 | CODE §3.4、ARCHITECTURE §4.3 | 已回填主要签名/字段，归一细节待补 |
| body markup 原样透传、actions 能力不强制 | ARCHITECTURE §4.3 | 待补 |
| `NoticeIcon::Name` 同时承载主题名/路径/URI | ARCHITECTURE §4.3 | 部分回填 |
| 无 D-Bus 激活服务文件 | BUILD | 待补 |
| 16 位图像与 `icon-multi` 不支持 | ARCHITECTURE §4.3 | 待补 |
| 示例 stdin 异步桥接、dunstify 参数格式 | BUILD | 已回填主要用法 |

## 8. 后续可补的实现项

- `Session::stop()` 或 `Drop` 清理，显式注销接口并释放总线名；
- 把 `image-path` 的 `file://` URI 归一为本地路径，或增加 `NoticeIcon::Path`；
- 16 位图像与多帧 `icon-multi` 模型；
- 对 `ActionInvoked` / `ActivationToken` 做在册 id 与动作键校验（如果需要更严格的宿主保护）；
- 在 host API 层暴露协议层在册 id 的只读查询，便于调试与恢复。
