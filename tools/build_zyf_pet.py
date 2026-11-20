"""把 zyf 素材文件夹（审阅版 GIF + 关键帧 PNG）打成本桌宠的动画包。

用法：python -I tools/build_zyf_pet.py <zyf素材文件夹> dist/pets/zyf
需要 Python + Pillow + numpy。
"""
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageSequence

SRC = Path(sys.argv[1])
OUT = Path(sys.argv[2])
CELL = (420, 390)       # 关键帧 PNG 的格子大小
BG = np.array([243, 242, 239])  # GIF 的底色
OUT_SCALE = 2 / 3       # 预先用 Lanczos 缩小，程序里再缩放时就不会糊
PACK_SCALE = 0.75       # 默认显示大小（相对上面缩小后的帧）
MARGIN = 4


def dilate(m):
    out = m.copy()
    out[1:] |= m[:-1]
    out[:-1] |= m[1:]
    out[:, 1:] |= m[:, :-1]
    out[:, :-1] |= m[:, 1:]
    return out


def key_out(rgb):
    """GIF 没有透明通道：从边缘洪泛掉底色，封闭的底色洞也去掉，再给描边外一圈做半透明去色边。"""
    d = np.abs(rgb.astype(int) - BG).sum(-1)
    near = d < 48
    border = np.zeros_like(near)
    border[0], border[-1], border[:, 0], border[:, -1] = True, True, True, True
    gone = border & near
    while True:
        grown = dilate(gone) & near
        if (grown == gone).all():
            break
        gone = grown
    gone |= d <= 8
    alpha = np.where(gone, 0.0, 1.0)
    ring = dilate(gone) & ~gone
    a = np.clip(d / 140.0, 0.25, 1.0)
    alpha[ring] = a[ring]
    rgbf = rgb.astype(float)
    av = alpha[..., None]
    fixed = np.where(av > 0, (rgbf - (1 - av) * BG) / np.maximum(av, 1e-6), 0)
    out = np.dstack([np.clip(fixed, 0, 255), alpha * 255]).astype("uint8")
    return Image.fromarray(out, "RGBA")


def gif(rel):
    frames, durs = [], []
    with Image.open(SRC / rel) as im:
        for f in ImageSequence.Iterator(im):
            frames.append(key_out(np.asarray(f.convert("RGB"))))
            durs.append(f.info.get("duration", 150))
    return frames, durs


def strip(rel, count, gif_rel):
    with Image.open(SRC / rel) as im:
        im = im.convert("RGBA")
        cols = im.width // CELL[0]
        frames = [im.crop((i % cols * CELL[0], i // cols * CELL[1],
                           i % cols * CELL[0] + CELL[0], i // cols * CELL[1] + CELL[1]))
                  for i in range(count)]
    with Image.open(SRC / gif_rel) as g:
        durs = [f.info.get("duration", 150) for f in ImageSequence.Iterator(g)]
    assert len(durs) == count, (rel, len(durs))
    return frames, durs


def float_idle(base, n=12, amp=3):
    """原素材没有待机：用挥手第一帧做一个上下漂浮的呼吸动画。"""
    frames = []
    for i in range(n):
        dy = round(-amp * (1 - np.cos(2 * np.pi * i / n)) / 2 / OUT_SCALE)
        c = Image.new("RGBA", base.size)
        c.alpha_composite(base, (0, max(dy, -base.getbbox()[1])))
        frames.append(c)
    return frames, [130] * n


def walk(direction):
    """左右飘行：从 6 姿势原图抠出主体，以红珠项链为锚点锁住身体，叠加上下起伏。
    姿势顺序、起伏、时长与 v01-walk-review/assemble_motion.py 一致，只是按本包的分辨率重新缩放。"""
    from scipy import ndimage

    sequence = [0, 1, 2, 3, 3, 4, 5, 0]
    bob = [0, -4, -8, -5, -1, 3, 2, 0]           # 192×208 图集里的像素
    durations = [120, 120, 120, 120, 120, 120, 120, 220]
    k = 420 / 180                                 # 图集像素 → 本脚本的 420 宽源像素
    scale = 0.37 * k
    sheet = Image.open(SRC / f"v01-walk-review/{direction}-source.png").convert("RGBA")
    poses = []
    for i in range(6):
        x, y = i % 3 * 512, i // 3 * 512
        rgba = np.asarray(sheet.crop((x, y, x + 512, y + 512))).copy()
        alpha = rgba[:, :, 3]
        labels, count = ndimage.label(alpha >= 128)
        sizes = np.bincount(labels.ravel(), minlength=count + 1)
        sizes[0] = 0
        core = labels == sizes.argmax()
        near = ndimage.binary_dilation(core, iterations=2)
        clean = np.clip((alpha.astype(np.float32) - 24) * 255 / 222, 0, 255)
        rgba[:, :, 3] = np.where(near, clean, 0).astype("uint8")
        img = Image.fromarray(rgba)
        b = img.getchannel("A").getbbox()
        red = core & (rgba[:, :, 0] > 120) & (rgba[:, :, 0] > rgba[:, :, 1] * 1.65)
        red[:max(b[3] - 153, 0)] = False
        beads, n = ndimage.label(red)
        bs = np.bincount(beads.ravel(), minlength=n + 1)
        bs[0] = 0
        ay, ax = ndimage.center_of_mass(beads == bs.argmax())
        poses.append((img.crop(b), (ax - b[0], ay - b[1])))
    max_left = max(round(a[0] * scale) for _, a in poses)
    max_right = max(round((s.width - a[0]) * scale) for s, a in poses)
    pivot_x = round((CELL[0] + max_left - max_right) / 2)
    frames = []
    for idx, off in zip(sequence, bob):
        sprite, (ax, ay) = poses[idx]
        resized = sprite.resize((round(sprite.width * scale), round(sprite.height * scale)), Image.Resampling.LANCZOS)
        frame = Image.new("RGBA", CELL)
        frame.alpha_composite(resized, (pivot_x - round(ax * scale), 300 + round(off * k) - round(ay * scale)))
        frames.append(frame)
    return frames, durations


wave, wave_d = gif("v03-wave-review/zyf-idle-wave-repaired.gif")
anims = {
    "待机": float_idle(wave[0]),
    "挥手": (wave, wave_d),
    "不屑": gif("v01-failed-review/zyf-failed-disdain.gif"),
    "看书": gif("v04-work-review/zyf-working-solid-glasses.gif"),
    "黑池跃出": strip("v05-jumping-review/zyf-black-pool-fluffy-front-rise-keyframes.png", 17,
                      "v05-jumping-review/zyf-black-pool-fluffy-front-rise.gif"),
    "等你确认": strip("v02-waiting-review/zyf-waiting-hair-blink-clock-keyframes.png", 8,
                      "v02-waiting-review/zyf-waiting-hair-blink-clock.gif"),
    "月下": strip("v04-review-moon/zyf-review-moon-face-orb-keyframes.png", 15,
                  "v04-review-moon/zyf-review-moon-face-orb.gif"),
    "向左飘": walk("left"),
    "向右飘": walk("right"),
}

# 每段动画整体（不是逐帧）按内容底边对齐到同一基线，横向保持原位，避免切换时跳动。
boxes = {}
for name, (frames, _) in anims.items():
    bb = [f.getbbox() for f in frames if f.getbbox()]
    boxes[name] = (min(b[0] for b in bb), min(b[1] for b in bb),
                   max(b[2] for b in bb), max(b[3] for b in bb))
tallest = max(b[3] - b[1] for b in boxes.values())
left = min(b[0] for b in boxes.values()) - MARGIN
right = max(b[2] for b in boxes.values()) + MARGIN
W, H = right - left, tallest + 2 * MARGIN
base_y = H - MARGIN
ow, oh = round(W * OUT_SCALE), round(H * OUT_SCALE)

(OUT / "anims").mkdir(parents=True, exist_ok=True)
files = {
    "待机": "idle", "挥手": "wave", "不屑": "disdain", "看书": "reading",
    "黑池跃出": "pool-rise", "等你确认": "waiting", "月下": "moon",
    "向左飘": "walk-left", "向右飘": "walk-right",
}
meta_anims = {}
for name, (frames, durs) in anims.items():
    dy = base_y - boxes[name][3]
    cols = min(len(frames), 8)
    rows = -(-len(frames) // cols)
    sheet = Image.new("RGBA", (ow * cols, oh * rows))
    for i, f in enumerate(frames):
        canvas = Image.new("RGBA", (W, H))
        canvas.alpha_composite(f.crop((left, 0, right, f.height)), (0, dy))
        small = canvas.resize((ow, oh), Image.Resampling.LANCZOS)
        sheet.alpha_composite(small, (i % cols * ow, i // cols * oh))
    path = f"anims/{files[name]}.png"
    sheet.save(OUT / path, optimize=True)
    meta_anims[name] = {"file": path, "frames": len(frames), "columns": cols, "durations": durs}
    print(f"{name}: {len(frames)} 帧 {ow}x{oh}")

pet = {
    "name": "振翼发",
    "credit": "JJkun28 制作的宝可梦振翼发同人像素形象：左右飘行、从黑水潭探头、戴红色圆墨镜翻书、凝聚月光发射紫色光球。",
    "scale": PACK_SCALE,
    "pixelated": False,
    "animations": meta_anims,
    "states": {
        "idle": ["待机"],
        "thinking": ["月下"],
        "working": ["看书"],
        "result": ["月下"],
        "waiting": ["等你确认"],
        "success": ["黑池跃出"],
        "error": ["不屑"],
        "drag": ["不屑"],
        "drag_left": ["向左飘"],
        "drag_right": ["向右飘"],
        "click": ["挥手", "黑池跃出"],
    },
    "idle_extras": {"min_secs": 30, "max_secs": 90, "pool": ["挥手", "看书", "黑池跃出", "向左飘", "向右飘"]},
    "menu": [
        {"group": "工作状态", "items": ["月下", "看书", "等你确认", "黑池跃出", "不屑"]},
        {"group": "小动作", "items": ["待机", "挥手", "向左飘", "向右飘"]},
    ],
}
(OUT / "pet.json").write_text(json.dumps(pet, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
