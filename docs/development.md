# 开发

## 从源码编译

需要 Rust。GNU 工具链（`stable-x86_64-pc-windows-gnu`）或 MSVC 工具链都可以，用 GNU 就不用装 Visual Studio：

```bash
cargo build --release
cargo test
```

产物在 `target/release/claude-pet.exe`，复制到 `dist/` 就能和动画包一起运行（桌宠开着时 exe 被占用，先右键「彻底退出」）。
exe 不提交进仓库（已在 `.gitignore` 里），给别人用的成品走 Releases。

## 发版

推一个 `v` 开头的 tag，GitHub Actions（`.github/workflows/release.yml`）会自动编译、跑测试，
把 exe、`dist/pets/`、README、LICENSE 打成 `claude-pet-<版本>-win64.zip`，发布到 Releases：

```bash
git tag v0.1.0
git push origin v0.1.0
```

本地也能打同样的包：

```powershell
cargo build --release
powershell -ExecutionPolicy Bypass -File tools\package.ps1 -Version v0.1.0
```

CI 用 MSVC 工具链编译，并静态链接 C 运行库，用户电脑上不需要装 VC++ 运行库。

## 代码结构

| 文件 | 内容 |
|---|---|
| `src/main.rs` | 命令行入口：启动 / `hook` / `install` / `uninstall` / `sessions` |
| `src/app.rs` | 窗口、渲染、动画播放、拖拽、右键菜单、对话框发送流程 |
| `src/chat.rs` | 对话输入框窗口 |
| `src/desktop.rs` | 和 Claude 桌面版打交道：读会话列表、`claude://` 跳转、找窗口、模拟打字（含单元测试） |
| `src/state.rs` | 会话状态聚合和优先级（含单元测试） |
| `src/hook.rs` | hook 客户端：读 stdin、找 Claude 进程、UDP 转发、自动拉起桌宠 |
| `src/install.rs` | 读写 `~/.claude/settings.json`，文件夹移动后自动修正路径（含单元测试） |
| `src/pack.rs` | 动画包加载（pet.json / 按文件名自动识别）、PNG 与 GIF 解码、缩放绘制（含单元测试） |
| `src/tray.rs` | 托盘图标：用宠物第一帧生成图标、左键显示隐藏、右键菜单（含单元测试） |
| `src/config.rs` | `config.json`；exe 目录不可写时改存 `%APPDATA%\claude-pet` |

## 工作原理

Claude Code 每触发一个 hook 事件，就运行一次 `claude-pet.exe hook`。这个进程从 stdin 读事件 JSON，
挑出事件名、会话 ID 等几个字段，用 UDP 发到本机 `127.0.0.1:47615`，然后立刻以 0 退出，不会阻塞 Claude Code。
桌宠本体在这个端口上收消息，按会话汇总状态后切换动画。桌宠没开时，hook 进程会顺手把它拉起来。

在 Git Bash 下每次 hook 调用约 0.25 秒，主要花在 bash 启动上，桌宠本身约 30 毫秒。

## README 预览动图

`docs/preview.gif` 由 `tools/make_preview.py` 用动画包的精灵图生成（需要 Pillow）。改了动画包后可以重新生成：

```bash
python -I tools/make_preview.py dist/pets docs/preview.gif
```
