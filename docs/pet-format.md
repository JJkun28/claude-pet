# 自己做动画包

每个动画包是 `pets/` 下的一个文件夹，里面一个 `pet.json` 加一个放精灵图的 `anims/`。
放进去以后，在右键「换宠物」里就能选到。

## pet.json

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

## 素材转换脚本

都在 `tools/` 里，需要 Python 和 Pillow。

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
