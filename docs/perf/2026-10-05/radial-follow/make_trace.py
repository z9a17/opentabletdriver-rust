"""Writes trace.csv: 6,000 synthetic PTH-660 positions (x, y, pause before
the report in ms) with slow circles, fast jumps, still jitter and three pen
lifts longer than Radial Follow's 50 ms reset. Seeded and deterministic."""
import math, random
random.seed(7)
rows = []
x, y = 22000.0, 5000.0
for i in range(6000):
    gap = 80 if i in (1500, 3000, 4500) else 0
    phase = i % 600
    if phase < 200:
        a = i * 0.03
        tx, ty = 22000 + 300 * math.cos(a), 5000 + 200 * math.sin(a)
    elif phase < 400:
        k = (i // 40) % 4
        tx, ty = [18000, 26000, 21000, 24000][k], [3000, 7000, 6500, 3500][k]
    else:
        tx, ty = 23000, 5500
    x += (tx - x) * 0.25
    y += (ty - y) * 0.25
    rows.append((int(round(x + random.gauss(0, 1.5))), int(round(y + random.gauss(0, 1.5))), gap))
open('trace.csv', 'w').write('\n'.join(f'{a},{b},{g}' for a, b, g in rows) + '\n')
