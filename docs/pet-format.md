# 自己做动画包

每个动画包是 `pets/` 下的一个文件夹。放进去以后，右键「重新加载配置和动画」，再到「换宠物」里就能选到。

## 最简单：放几个 GIF 就行

不用写任何配置。在 `pets/` 下新建一个文件夹，把 GIF 放进去，**按状态给文件命名**：

```
pets/我的宠物/
  idle.gif        待机（必须有一个；没有的话拿第一个动画当待机）
  thinking.gif    思考中
  working.gif     工作中
  waiting.gif     等待你确认
  success.gif     完成
  error.gif       出错
  click-1.gif     单击时（同一状态可以放好几个，随机播）
  click-2.gif
  wave.gif        认不出状态的，当作待机时的随机小动作
```

- 文件名认这些（大小写都行）：

  | 状态 | 英文名 | 中文名 |
  |---|---|---|
  | 待机 | `idle` | 待机 |
  | 思考中 | `thinking`、`think` | 思考 |
  | 工作中 | `working`、`work` | 工作 |
  | 整理结果 | `result` | 整理 |
  | 等待你确认 | `waiting`、`wait` | 等待 |
  | 完成 | `success`、`done` | 完成 |
  | 出错 | `error`、`fail`、`failed` | 出错、失败 |
  | 拖动时 | `drag`（朝左 / 朝右：`drag_left`、`drag_right`） | 拖动、拖拽（向左、向右） |
  | 单击时 | `click` | 点击 |

  英文名后面可以接 `-`、`_`、空格或数字，例如 `click-2.gif`、`idle_1.gif`；中文名只要开头对上就行，例如「工作中.gif」「等待你确认.gif」。
- 没放的状态会自动回退（整理结果 → 工作中 → 待机），所以只放一个 `idle.gif` 也能用。
- 除了 GIF，也可以放一个**装着 PNG 帧的文件夹**，文件夹名就是动画名，例如 `working/001.png、002.png…`。
  帧按文件名的自然顺序播（`2.png` 在 `10.png` 前面），每帧 1/12 秒，所有帧的尺寸要一样。
  PNG 支持半透明，边缘比 GIF 平滑。
- GIF 和帧文件夹放在宠物文件夹里，或者它的 `anims/` 子文件夹里都行。
- 默认大小按待机动画的高度自动定，显示出来大约 160 像素高，可以在右键「大小」里改。
  待机动画不到 96 像素高的会当成像素画，放大时保持像素锐利。
- GIF 要用**透明背景**。GIF 的透明只有「全透明」和「不透明」两种，平滑画风的边缘可能有一圈锯齿；介意的话用 PNG 帧文件夹。

想调整播放速度、菜单分组、随机小动作的间隔，再写一个 `pet.json`（见下面）。`pet.json` 里的动画同样可以直接写 GIF 或帧文件夹。

## pet.json

```jsonc
{
  "name": "我的宠物",
  "scale": 1.5,                       // 默认显示大小
  "pixelated": false,                 // 像素画设 true：放大时保持像素锐利，不做平滑
  "animations": {                     // 每段动画一张精灵图，帧按网格从左到右、从上到下排
    "待机": { "file": "anims/idle.png", "frames": 24, "columns": 8, "fps": 12 },
    "挥手": { "file": "anims/wave.png", "frames": 4, "durations": [140, 140, 140, 280] }, // 逐帧毫秒，覆盖 fps
    "敲键盘": { "file": "anims/typing.png", "frames": 16, "columns": 8, "fps": 12 },
    "转圈": { "file": "anims/spin.gif" },              // GIF：帧数和每帧时长从 GIF 里读
    "跑步": { "file": "anims/run/", "fps": 10 }        // PNG 帧文件夹：帧数自己数，按 fps 播
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

## 素材转换脚本

现在 GIF 和 PNG 帧文件夹可以直接用，一般用不到这些脚本了。它们适合要精细处理素材的情况
（抠背景、对齐基线、转换 Codex 宠物包），都在 `tools/` 里，需要 Python 和 Pillow。

`tools/convert_desk_pet.py` 把 desk-pet 的预览 GIF 转成上面的格式：

```bash
python -I tools/convert_desk_pet.py <desk-pet仓库>/desk-pet/assets dist/pets/maid
```

想用自己的 GIF，可以照着这个脚本里的 `gif_to_sheet` 函数改。

`tools/convert_codex_pet.py` 把 Codex 宠物包（`pet.json` + `spritesheet.webp`，192×208 格、8 列图集，第 1、2 版都行）转过来，
逐帧时长和像素画模式都会带上。Codex 第 2 版的十六方向注视会合成一段「转一圈」动作（暂不支持跟随鼠标转头）：

```bash
python -I tools/convert_codex_pet.py <Codex宠物文件夹> dist/pets/<新名字>
```

`tools/build_zyf_pet.py` 把振翼发的 zyf 素材文件夹（各动作审阅版 GIF + 关键帧 PNG，左右飘行取 `v01-walk-review` 的原图）打成 `dist/pets/zyf`（另外需要 numpy + scipy）。
GIF 的浅灰底色会自动抠掉；所有动作按同一基线对齐，并预先缩到 2/3，默认显示大小 0.75：

```bash
python -I tools/build_zyf_pet.py <zyf素材文件夹> dist/pets/zyf
```
