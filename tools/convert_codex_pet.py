"""把 Codex 宠物包（pet.json + spritesheet.webp，8 列图集）转成本桌宠的动画包。

用法（需要 Python + Pillow）：
    python -I tools/convert_codex_pet.py <Codex 宠物文件夹> dist/pets/<新名字>

Codex 图集规格：每格 192×208，8 列；第 1 版 9 行，第 2 版多 2 行十六方向注视。
每行一段动画，帧数和逐帧时长按 Codex 的标准写死在下面的 ROWS 里。
输出：每段动画一张横条 PNG（颜色不超过 256 种时存成调色板图，无损且更省内存）。
"""
import json
import sys
from pathlib import Path

from PIL import Image

CELL_W, CELL_H, COLS = 192, 208, 8

# (行号, codex 名, 中文名, 逐帧毫秒)
ROWS = [
    (0, "idle", "待机", [280, 110, 110, 140, 140, 320]),
    (1, "running-right", "向右跑", [120] * 7 + [220]),
    (2, "running-left", "向左跑", [120] * 7 + [220]),
    (3, "waving", "挥手", [140, 140, 140, 280]),
    (4, "jumping", "跳起来", [140, 140, 140, 140, 280]),
    (5, "failed", "失败", [140] * 7 + [240]),
    (6, "waiting", "等你确认", [150] * 5 + [260]),
    (7, "running", "工作中", [120] * 5 + [220]),
    (8, "review", "检查", [150] * 5 + [280]),
]
LOOK = (9, 10, "转一圈", [160] * 7 + [260] + [160] * 7 + [260])  # 第 2 版：两行合成一圈

STATES = {
    "idle": ["待机"],
    "thinking": ["检查"],
    "working": ["工作中"],
    "result": ["检查"],
    "waiting": ["等你确认"],
    "success": ["跳起来", "挥手"],
    "error": ["失败"],
    "drag": ["向右跑"],          # 上下拖、还没分出左右时
    "drag_left": ["向左跑"],
    "drag_right": ["向右跑"],
    "click": ["挥手", "跳起来"],
}
IDLE_EXTRAS = ["挥手", "转一圈", "向左跑", "向右跑", "跳起来"]


def cells(atlas, row, n):
    return [atlas.crop((c * CELL_W, row * CELL_H, (c + 1) * CELL_W, (row + 1) * CELL_H)) for c in range(n)]


def save_strip(frames, path):
    strip = Image.new("RGBA", (CELL_W * len(frames), CELL_H), (0, 0, 0, 0))
    for i, f in enumerate(frames):
        strip.paste(f, (i * CELL_W, 0))
    # 完全透明的像素统一成一种颜色，减少调色板占用
    data = [p if p[3] else (0, 0, 0, 0) for p in strip.getdata()]
    strip.putdata(data)
    colors = strip.getcolors(256)
    if colors:  # ≤256 色：无损存成调色板 PNG
        palette = [c for _, c in colors]
        index = {c: i for i, c in enumerate(palette)}
        img = Image.new("P", strip.size)
        img.putdata([index[p] for p in data])
        flat = []
        for r, g, b, _ in palette:
            flat += [r, g, b]
        img.putpalette(flat)
        img.save(path, optimize=True, transparency=bytes(a for *_, a in palette))
        return "调色板"
    strip.save(path, optimize=True)
    return "RGBA"


def main(src, dst):
    src, dst = Path(src), Path(dst)
    meta = json.loads((src / "pet.json").read_text(encoding="utf-8"))
    atlas = Image.open(src / meta.get("spritesheetPath", "spritesheet.webp")).convert("RGBA")
    rows = atlas.height // CELL_H
    if atlas.width != CELL_W * COLS or rows < 9:
        sys.exit(f"图集尺寸 {atlas.size} 不是 Codex 规格（{CELL_W * COLS} 宽、至少 9 行 × {CELL_H}）")

    (dst / "anims").mkdir(parents=True, exist_ok=True)
    animations = {}

    def add(name, file, frames, durations):
        kind = save_strip(frames, dst / "anims" / file)
        animations[name] = {"file": f"anims/{file}", "frames": len(frames), "columns": len(frames), "durations": durations}
        print(f"  {name}: {len(frames)} 帧（{kind}）")

    for row, key, name, dur in ROWS:
        add(name, f"{key}.png", cells(atlas, row, len(dur)), dur)
    if rows >= 11:
        r1, r2, name, dur = LOOK
        add(name, "look.png", cells(atlas, r1, 8) + cells(atlas, r2, 8), dur)

    has = lambda n: n in animations
    pack = {
        "name": meta.get("displayName") or meta.get("id") or src.name,
        "credit": "由 Codex 宠物包转换：" + (meta.get("description") or ""),
        "scale": 1.0,
        "pixelated": True,
        "animations": animations,
        "states": {k: [n for n in v if has(n)] for k, v in STATES.items()},
        "idle_extras": {"min_secs": 30, "max_secs": 90, "pool": [n for n in IDLE_EXTRAS if has(n)]},
        "menu": [
            {"group": "工作状态", "items": [n for n in ["检查", "工作中", "等你确认", "跳起来", "失败"] if has(n)]},
            {"group": "小动作", "items": [n for n in ["待机", "挥手", "转一圈", "向左跑", "向右跑"] if has(n)]},
        ],
    }
    (dst / "pet.json").write_text(json.dumps(pack, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"完成：{pack['name']}，{len(animations)} 段动画 → {dst}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
