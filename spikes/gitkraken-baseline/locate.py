"""Find the first file row in GitKraken's commit panel from a screenshot of it."""
import struct, subprocess, sys, tempfile, zlib

def read_png(path):
    b = open(path, 'rb').read(); i = 8; idat = b''
    while i < len(b):
        n, t = struct.unpack('>I4s', b[i:i+8])
        if t == b'IHDR': w, h, depth, ct = struct.unpack('>IIBB', b[i+8:i+18])
        if t == b'IDAT': idat += b[i+8:i+8+n]
        i += 12 + n
    if depth != 8 or ct not in (2, 6): raise SystemExit(f'unsupported png {depth}/{ct}')
    bpp = 3 if ct == 2 else 4; s = w * bpp; raw = zlib.decompress(idat); out = []; prev = bytearray(s); p = 0
    for _ in range(h):
        f = raw[p]; line = bytearray(raw[p+1:p+1+s]); p += s + 1
        for x in range(s):
            a = line[x-bpp] if x >= bpp else 0; c = prev[x]; d = prev[x-bpp] if x >= bpp else 0
            if f == 1: line[x] = (line[x] + a) & 255
            elif f == 2: line[x] = (line[x] + c) & 255
            elif f == 3: line[x] = (line[x] + (a + c) // 2) & 255
            elif f == 4:
                e = a + c - d; pa, pb, pc = abs(e-a), abs(e-c), abs(e-d)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else c if pb <= pc else d)) & 255
        out.append(line); prev = line
    return w, h, bpp, out

X, Y, W, H = 1383, 400, 45, 620   # pencil-icon column of the panel, in points
def first_file_row():
    """Return the y in points of the first row below the Path/Tree bar, or None."""
    f = tempfile.mktemp(suffix='.png')
    subprocess.run(['screencapture', '-x', f'-R{X},{Y},{W},{H}', f], check=True)
    w, h, bpp, px = read_png(f); sc = w / W
    # Orange/yellow pencil icons sit in a narrow column; group hits into horizontal bands.
    def hits(x0, x1):
        ys = []
        for y in range(h):
            r = px[y]
            if any(r[x*bpp] > 200 and 140 < r[x*bpp+1] < 215 and r[x*bpp+2] < 110 for x in range(int(x0*sc), int(x1*sc))): ys.append(y)
        bands = []
        for y in ys:
            if bands and y - bands[-1][1] <= 3: bands[-1][1] = y
            else: bands.append([y, y])
        return [(a + b) / 2 / sc + Y for a, b in bands if 6 * sc <= b - a <= 15 * sc]
    return hits(0, 45)

if __name__ == '__main__': print(first_file_row())
