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
