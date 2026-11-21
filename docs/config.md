# 设置（config.json）

`config.json` 在 `claude-pet.exe` 同一个文件夹里，第一次运行时自动生成。改完在右键菜单点「重新加载配置和动画」即可，不用重启。

如果 exe 所在的文件夹没有写入权限（例如放进了 `C:\Program Files`），`config.json` 和 `hook-events.log`
会改存到 `%APPDATA%\claude-pet`。

| 字段 | 默认 | 说明 |
|---|---|---|
| `pet` | `"ghost-squeak"` | 使用 `pets/` 下哪个动画包。指定的包不存在时，自动换成默认包或第一个能用的包 |
| `scale` | 动画包自带 | 显示大小（右键「大小」会写这里） |
| `topmost` | `true` | 始终置顶 |
| `idle_extras` | `true` | 待机时随机播小动作 |
| `success_secs` / `error_secs` | `10` / `12` | 完成 / 出错动画展示多久 |
| `hold_ms` | `600` | 状态降级前的最短停留 |
| `stale_minutes` | `0` | 会话多少分钟无事件就清掉，0 = 不超时 |
| `auto_launch` | `true` | 桌宠没开时，收到 hook 事件就自动启动它 |
| `quit_by_user` | `false` | 右键「彻底退出」会设为 `true`，暂停自动启动；手动打开一次就恢复 |
| `claude_app` | `""` | 「打开 Claude Code」启动什么，空 = 自动找 Claude 桌面版；也可以填 exe 路径 |
| `hook_log` | `true` | 记录 hook 事件到 `hook-events.log`（超过 512 KB 自动清空重写） |
| `chat_auto_send` | `true` | 对话框打完字自动按回车；`false` = 只把字打进 Claude 的输入框，你自己按回车 |
| `chat_delay_ms` | `600` | 切到会话后等多久再打字（毫秒）。电脑慢、出现字没打进去的情况就调大 |
| `tray` | `true` | 在任务栏右下角显示托盘图标 |
| `port` | `47615` | hook 与桌宠通信的本机 UDP 端口 |
