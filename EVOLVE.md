# EVOLVE — 需求演进记录

## 1. 完善控制中心

- 需求：重做控制中心的媒体控制、音量/亮度滑块与日历：媒体按钮改用符号图标并横向平铺满整行、底部对齐，播放/暂停图标随 MPRIS 播放状态切换且尺寸更大；滑块不要圆点、轨道用胶囊大圆角，轨道取 `muted` 色、填充取 `fg` 色，文字与滑块同行且文字在左；日历增加 `surface` 色卡片背景并高亮今天。

- 实现思路：
  - `SystemState` 增加 `media: Option<MediaState>`，`MediaState` 只承载 `playing`；保留 `None` 表示总线上没有 MPRIS 播放器。
  - daemon 的 system 运行时抽出 `player_bus_name` 复用播放器选择逻辑；新增 `poll_media` / `media_read`，每 `MEDIA_POLL = 2s` 读取 `org.mpris.MediaPlayer2.Player.PlaybackStatus`，仅在变化时发送 `DaemonEvent::System`。
  - 媒体控制改用手绘按钮 + `gtk::Image::from_icon_name` 符号图标（上一首 / 播放暂停 / 下一首），不再使用 emoji；`GtkBox` 设 `homogeneous` + `hexpand` 铺满整行，按钮与图标 `valign = End`，播放暂停像素尺寸 22、其余 16。
  - 播放/暂停按钮保存 `Rc<Cell<bool>>` 状态与图标句柄：点击时乐观切换图标再下发 `MediaOp::PlayPause`，`set_system` 收到轮询结果后校正。
  - 滑块行由纵向改为横向：标签固定宽度、`xalign = 0`，`GtkScale` 横向扩展；CSS 针对 `.center-popover scale` 重绘轨道与填充，轨道取 `muted`、填充取 `fg`，均为 999px 胶囊圆角。
  - 去掉滑块圆点：`scale slider` 尺寸归零并设为透明；同时用 `background-image: none`、`border: none`、`box-shadow: none` 覆盖 Breeze 主题的渐变与描边，避免主题色/主题圆点残留；`GtkRange` 本身仍处理整条轨道的点击与拖动。
  - 日历沿用 `GtkCalendar`，CSS 给 `.center-calendar` 加 `surface` 背景、圆角与 `gold` 细边；利用 GTK 自带的 `.today` / `.other-month` / `:selected` 节点状态着色。

- 文档操作：

| 文档 | 章节 | 操作 | 内容 |
|------|------|------|------|
| `USER.html` | §5.1 样式基础变量 | 改 | `muted` 说明补充滑块轨道，`fg` 说明补充滑块填充。 |
| `USER.html` | §5.2.4 控制中心 | 改 | 补充媒体符号图标与布局、播放状态切换、滑块 muted 轨道 / fg 填充与无圆点交互、日历 surface 卡片与今天高亮。 |
| `CORE.html` | §4.17 `bar/modules/center.rs` | 改 | 更新 `CenterWidget` 字段；增加媒体按钮、滑块、日历的实现注记。 |
| `CORE.html` | §4.5 `daemon/system.rs` | 改 | 补充 `MEDIA_POLL = 2s` 与 MPRIS `PlaybackStatus` 周期采样。 |
| `CORE.html` | §4.19 `shared/model.rs` | 改 | `SystemState` 增加 `media`，新增 `MediaState { playing }`。 |
| `TODO.md` | 完善控制中心 | 改 | 标记 `[已解决]`。 |
