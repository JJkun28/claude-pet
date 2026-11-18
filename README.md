# Claude 桌宠（原生 Windows 版）

一个跟着 Claude Code 实时切换动作的桌面宠物。纯 Win32 + Rust，不用 Electron，也不用浏览器内核：
单个 exe 约 0.7 MB，运行时内存约 12 MB（私有内存）。

## 怎么用

成品都在 `dist/` 文件夹里，整个文件夹可以拷到任何地方（便携版，不用安装）：

```
dist/
  claude-pet.exe        桌宠本体（双击启动）
  config.json           程序设置（首次运行自动生成）
  hook-events.log       收到的 hook 事件日志（排查用）
  pets/maid/pet.json    动画包：动作定义
  pets/maid/anims/*.png 动画包：精灵图
```

1. 双击 `claude-pet.exe`，桌宠出现在屏幕右下角。
2. 第一次启动会问「要不要接入 Claude Code」，点「是」即可（会先备份 `~/.claude/settings.json`）。
   以后也可以在右键菜单里随时接入或断开。
3. **新开**一个 Claude Code 会话，桌宠就会跟着动了。

> 移动了 `dist` 文件夹后，要在右键菜单里把「接入 Claude Code」关掉再打开一次，hooks 里记录的是 exe 的完整路径。

### 操作

- **左键拖动**：移动位置（自动记住）。
- **左键单击**：随机播一个点击回应动作。
- **右键**：菜单。
  - 顶部显示当前状态和进行中的会话数，下面是「打开 Claude Code」（没开就启动，开着就切到前台）。
  - 「工作状态 / 小动作 / 玩耍 / …」：列出动画包里的全部动作，点哪个就播哪个，播完回到当前状态。
  - 「按状态预览」：预览思考、工作、等待等状态的动画。
  - 大小、换宠物（`pets/` 下有多个动画包时出现）、始终置顶、待机时随机小动作、开机自启。
  - 接入 Claude Code（hooks）、重置会话状态、查看 hook 事件日志、重新加载配置和动画、退出。

## 状态逻辑

| Claude Code 事件 | 桌宠状态 |
|---|---|
| UserPromptSubmit | 思考中 |
| PreToolUse、SubagentStart、PreCompact | 工作中（AskUserQuestion 工具算「等待你确认」） |
| PostToolUse、PostToolUseFailure | 整理结果 |
| PermissionRequest、Notification(permission_prompt) | 等待你确认 |
| Stop | 完成（展示 `success_secs` 秒后回待机） |
| StopFailure | 出错（展示 `error_secs` 秒后回待机） |
| Notification(idle_prompt) | 回待机（被 Esc 中断时 Claude Code 不发 Stop，靠这个） |
| SessionEnd，或该会话的 Claude 进程已退出 | 回待机 |

- 思考、工作、整理、等待会**一直保持**，直到收到下一个事件。不会因为超时被待机动作打断。
- 多个会话同时进行时，显示优先级最高的那个：**等待你确认 > 出错 > 工作中 > 思考中 > 整理结果 > 完成**。
  所以一个会话做完、另一个还在干活时，不会提前庆祝。
- 状态降级（例如 工作中→整理结果）前至少保持 `hold_ms` 毫秒，防止来回闪。
- 待机随机小动作和点击回应会被状态变化立即打断；右键点播的动作会完整播完。

## 设置（config.json）

| 字段 | 默认 | 说明 |
|---|---|---|
| `pet` | `"maid"` | 使用 `pets/` 下哪个动画包 |
| `scale` | 动画包自带 | 显示大小（右键「大小」会写这里） |
| `topmost` | `true` | 始终置顶 |
| `idle_extras` | `true` | 待机时随机播小动作 |
| `success_secs` / `error_secs` | `10` / `12` | 完成 / 出错动画展示多久 |
| `hold_ms` | `600` | 状态降级前的最短停留 |
| `stale_minutes` | `0` | 会话多少分钟无事件就清掉，0 = 不超时 |
| `auto_launch` | `true` | 桌宠没开时，新会话开始自动启动它 |
| `claude_app` | `""` | 「打开 Claude Code」启动什么，空 = 自动找 Claude 桌面版；也可以填 exe 路径 |
| `hook_log` | `true` | 记录 hook 事件到 `hook-events.log` |
| `port` | `47615` | hook 与桌宠通信的本机 UDP 端口 |

改完在右键菜单点「重新加载配置和动画」即可，不用重启。

## 动画包格式（pets/<名字>/pet.json）

```jsonc
{
  "name": "我的宠物",
  "scale": 1.5,                       // 默认显示大小
  "pixelated": false,                 // 像素画设 true：放大时保持像素锐利，不做平滑
  "animations": {                     // 每段动画一张精灵图，帧按网格从左到右、从上到下排
    "待机": { "file": "anims/idle.png", "frames": 24, "columns": 8, "fps": 12 },
    "挥手": { "file": "anims/wave.png", "frames": 4, "durations": [140, 140, 140, 280] }, // 逐帧毫秒，覆盖 fps
    "敲键盘": { "file": "anims/typing.png", "frames": 16, "columns": 8, "fps": 12 }
  },
  "states": {                         // 每个状态一个动画池，随机挑一段循环播
    "idle": ["待机"],
    "thinking": ["..."], "working": ["敲键盘"], "result": ["..."],
    "waiting": ["..."], "success": ["..."], "error": ["..."],
    "drag": ["..."],                  // 拖动时
    "click": ["..."]                  // 单击时
  },
  "idle_extras": { "min_secs": 30, "max_secs": 90, "pool": ["..."] },
  "menu": [                           // 右键菜单分组；不写就列出全部动作
    { "group": "日常", "items": ["待机", "敲键盘"] }
  ]
}
```

- **加新动作不用改代码**：把精灵图放进 `anims/`，在 `animations` 里加一行。想让它出现在菜单、某个状态或随机小动作里，就把名字写进 `menu`、`states` 或 `idle_extras.pool`，然后右键「重新加载配置和动画」。
- 某个状态没配动画时会自动回退，例如 result→working→idle，所以最少只配一个 `idle` 也能跑。
- 精灵图支持普通 RGBA PNG，也支持 8 位调色板 PNG。调色板图每像素只占 1 字节，更省内存。
- 内存里只保留最近用到的 3 段动画，动画再多也不会涨内存。

### 素材转换

`tools/convert_desk_pet.py` 把 desk-pet 的预览 GIF 转成上面的格式（需要 Python + Pillow）：

```bash
python -I tools/convert_desk_pet.py <desk-pet仓库>/desk-pet/assets dist/pets/maid
```

想用自己的 GIF，可以照着这个脚本里的 `gif_to_sheet` 函数改。

`tools/convert_codex_pet.py` 把 Codex 宠物包（`pet.json` + `spritesheet.webp`，192×208 格、8 列图集，第 1、2 版都行）转过来，
逐帧时长和像素画模式都会带上：

```bash
python -I tools/convert_codex_pet.py <Codex宠物文件夹> dist/pets/<新名字>
```

转完在右键「换宠物」里选它。Codex 第 2 版的十六方向注视会合成一段「转一圈」动作（暂不支持跟随鼠标转头）。

## 从源码编译

需要 Rust（GNU 工具链即可，不用装 Visual Studio）：

```bash
cargo build --release
cargo test
```

产物在 `target/release/claude-pet.exe`，复制到 `dist/` 覆盖即可。

代码结构：

| 文件 | 内容 |
|---|---|
| `src/main.rs` | 命令行入口：启动 / `hook` / `install` / `uninstall` |
| `src/app.rs` | 窗口、渲染、动画播放、拖拽、右键菜单 |
| `src/state.rs` | 会话状态聚合和优先级（含单元测试） |
| `src/hook.rs` | hook 客户端：读 stdin、找 Claude 进程、UDP 转发、自动拉起桌宠 |
| `src/install.rs` | 读写 `~/.claude/settings.json` |
| `src/pack.rs` | 动画包加载、PNG 解码、缩放绘制 |
| `src/config.rs` | `config.json` |

## 已知限制

- Windows 桌面端 Code 标签页里，`PostToolUse` 等部分 hook 可能不触发（anthropics/claude-code#97977）。少了它只会少显示「整理结果」，不影响其他状态。打开「查看 hook 事件日志」能看到实际收到了哪些事件。
- 在 Git Bash 下每次 hook 调用约 0.25 秒，主要花在 bash 启动上，桌宠本身约 30 毫秒。
- 暂不支持跟随 Codex 等其他工具的状态（Codex 的宠物美术可以转换过来用，见上文）。

## 素材版权

`dist/pets/maid` 的美术素材来自 [desk-pet](https://github.com/tg111/claude-code-desk-pet) / [dsh-pet](https://github.com/PC2005-cloud/dsh-pet)，**仅限个人非商用**，分发时须保留原作者地址 <https://github.com/PC2005-cloud/dsh-pet>。程序代码本身随意使用。

`dist/pets/ghost-squeak`（鬼马史魁克）是 JJkun28 制作的荒野乱斗同人像素形象，仅限非商用；角色版权归 Supercell，与 Supercell 无关联。
