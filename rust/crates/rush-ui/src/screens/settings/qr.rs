//! QR codes, for pairing a phone: text in the smallest version that holds
//! it, mask 0, exactly as Go's rsc.io/qr v0.2.0 draws them (qr.go:30
//! Encode, coding/qr.go: Bits, Num, Alpha, String, vtab, vplan, fplan,
//! lplan, mplan, posBox, alignBox; gf256/gf256.go: Field, RSEncoder).

/// How much of the code is error correction: the two levels rush uses,
/// numbered as Go's (L 0 and M 1 aren't needed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Q = 2,
    H = 3,
}

/// A square of modules, true for dark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Code {
    pub size: usize,
    dark: Vec<bool>,
}

impl Code {
    /// Whether the module at x, y is dark; outside the code it isn't.
    pub fn black(&self, x: isize, y: isize) -> bool {
        let n = self.size as isize;
        (0..n).contains(&x) && (0..n).contains(&y) && self.dark[(y * n + x) as usize]
    }
}

/// text as a QR code at level l (Encode): numeric, alphanumeric or bytes,
/// whichever is the smallest that takes it, in the smallest version.
pub fn encode(text: &str, l: Level) -> Result<Code, String> {
    let enc = if text.bytes().all(|c| c.is_ascii_digit()) {
        Enc::Num
    } else if text.chars().all(|c| ALPHABET.contains(c)) {
        Enc::Alpha
    } else {
        Enc::Bytes
    };
    let mut v = 1;
    loop {
        if v > 40 {
            return Err("text too long to encode as QR".into());
        }
        if enc.bits(text, v) <= data_bytes(v, l) * 8 {
            break;
        }
        v += 1;
    }
    let plan = Plan::new(v, l);
    let mut b = Bits::default();
    enc.write(text, &mut b, v);
    b.add_check_bytes(v, l);
    let bytes = b.b;
    let size = plan.pixel.len();
    let mut dark = vec![false; size * size];
    for (y, row) in plan.pixel.iter().enumerate() {
        for (x, &pix) in row.iter().enumerate() {
            let mut pix = pix;
            if matches!(role(pix), DATA | CHECK) {
                let o = (pix >> 6) as usize;
                if bytes[o / 8] & (1 << (7 - (o & 7))) != 0 {
                    pix ^= BLACK;
                }
            }
            dark[y * size + x] = pix & BLACK != 0;
        }
    }
    Ok(Code { size, dark })
}

const ALPHABET: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

#[derive(Clone, Copy)]
enum Enc {
    Num,
    Alpha,
    Bytes,
}

fn size_class(v: usize) -> usize {
    match v {
        0..=9 => 0,
        10..=26 => 1,
        _ => 2,
    }
}

impl Enc {
    fn len_bits(self, v: usize) -> usize {
        let t = match self {
            Enc::Num => [10, 12, 14],
            Enc::Alpha => [9, 11, 13],
            Enc::Bytes => [8, 16, 16],
        };
        t[size_class(v)]
    }

    fn bits(self, s: &str, v: usize) -> usize {
        let n = s.len();
        4 + self.len_bits(v)
            + match self {
                Enc::Num => (10 * n).div_ceil(3),
                Enc::Alpha => (11 * n).div_ceil(2),
                Enc::Bytes => 8 * n,
            }
    }

    fn write(self, s: &str, b: &mut Bits, v: usize) {
        let s = s.as_bytes();
        let mode = match self {
            Enc::Num => 1,
            Enc::Alpha => 2,
            Enc::Bytes => 4,
        };
        b.write(mode, 4);
        b.write(s.len() as u32, self.len_bits(v));
        match self {
            Enc::Num => {
                let d = |c: u8| (c - b'0') as u32;
                let (threes, rest) = s.as_chunks::<3>();
                for c in threes {
                    b.write(d(c[0]) * 100 + d(c[1]) * 10 + d(c[2]), 10);
                }
                match rest {
                    [a] => b.write(d(*a), 4),
                    [a, c] => b.write(d(*a) * 10 + d(*c), 7),
                    _ => {}
                }
            }
            Enc::Alpha => {
                let at = |c: u8| ALPHABET.find(c as char).unwrap_or(0) as u32;
                let (pairs, rest) = s.as_chunks::<2>();
                for p in pairs {
                    b.write(at(p[0]) * 45 + at(p[1]), 11);
                }
                if let [c] = rest {
                    b.write(at(*c), 6);
                }
            }
            Enc::Bytes => {
                for &c in s {
                    b.write(c as u32, 8);
                }
            }
        }
    }
}

#[derive(Default)]
struct Bits {
    b: Vec<u8>,
    nbit: usize,
}

impl Bits {
    fn write(&mut self, mut v: u32, mut nbit: usize) {
        while nbit > 0 {
            let mut n = nbit.min(8);
            if self.nbit.is_multiple_of(8) {
                self.b.push(0);
            } else {
                n = n.min(8 - self.nbit % 8);
            }
            self.nbit += n;
            let sh = nbit - n;
            let last = self.b.len() - 1;
            self.b[last] |= ((v >> sh) << ((8 - self.nbit % 8) % 8)) as u8;
            v -= (v >> sh) << sh;
            nbit -= n;
        }
    }

    fn pad(&mut self, mut n: usize) {
        if n <= 4 {
            self.write(0, n);
            return;
        }
        self.write(0, 4);
        n -= 4;
        let to_byte = (8 - self.nbit % 8) % 8;
        n -= to_byte;
        self.write(0, to_byte);
        let pad = n / 8;
        let mut i = 0;
        while i < pad {
            self.write(0xec, 8);
            if i + 1 >= pad {
                break;
            }
            self.write(0x11, 8);
            i += 2;
        }
    }

    fn add_check_bytes(&mut self, v: usize, l: Level) {
        let nd = data_bytes(v, l);
        if self.nbit < nd * 8 {
            self.pad(nd * 8 - self.nbit);
        }
        let (nblock, check) = VTAB[v].level[l as usize];
        let rs = Rs::new(check);
        let dat = self.b.clone();
        let mut at = 0;
        let extra = nd % nblock;
        let mut db = nd / nblock;
        for i in 0..nblock {
            if i == nblock - extra {
                db += 1;
            }
            let chk = rs.ecc(&dat[at..at + db]);
            self.b.extend_from_slice(&chk);
            self.nbit += 8 * chk.len();
            at += db;
        }
    }
}

/// A version's layout (version): where its alignment boxes go, its
/// bytes, its version pattern, and per level (blocks, check bytes each).
struct Version {
    apos: usize,
    astride: usize,
    bytes: usize,
    pattern: u32,
    level: [(usize, usize); 4],
}

const fn ver(
    apos: usize,
    astride: usize,
    bytes: usize,
    pattern: u32,
    level: [(usize, usize); 4],
) -> Version {
    Version {
        apos,
        astride,
        bytes,
        pattern,
        level,
    }
}

#[rustfmt::skip]
const VTAB: [Version; 41] = [
    ver(0, 0, 0, 0, [(1, 0); 4]),
    ver(100, 100, 26, 0x0, [(1, 7), (1, 10), (1, 13), (1, 17)]),
    ver(16, 100, 44, 0x0, [(1, 10), (1, 16), (1, 22), (1, 28)]),
    ver(20, 100, 70, 0x0, [(1, 15), (1, 26), (2, 18), (2, 22)]),
    ver(24, 100, 100, 0x0, [(1, 20), (2, 18), (2, 26), (4, 16)]),
    ver(28, 100, 134, 0x0, [(1, 26), (2, 24), (4, 18), (4, 22)]),
    ver(32, 100, 172, 0x0, [(2, 18), (4, 16), (4, 24), (4, 28)]),
    ver(20, 16, 196, 0x7c94, [(2, 20), (4, 18), (6, 18), (5, 26)]),
    ver(22, 18, 242, 0x85bc, [(2, 24), (4, 22), (6, 22), (6, 26)]),
    ver(24, 20, 292, 0x9a99, [(2, 30), (5, 22), (8, 20), (8, 24)]),
    ver(26, 22, 346, 0xa4d3, [(4, 18), (5, 26), (8, 24), (8, 28)]),
    ver(28, 24, 404, 0xbbf6, [(4, 20), (5, 30), (8, 28), (11, 24)]),
    ver(30, 26, 466, 0xc762, [(4, 24), (8, 22), (10, 26), (11, 28)]),
    ver(32, 28, 532, 0xd847, [(4, 26), (9, 22), (12, 24), (16, 22)]),
    ver(24, 20, 581, 0xe60d, [(4, 30), (9, 24), (16, 20), (16, 24)]),
    ver(24, 22, 655, 0xf928, [(6, 22), (10, 24), (12, 30), (18, 24)]),
    ver(24, 24, 733, 0x10b78, [(6, 24), (10, 28), (17, 24), (16, 30)]),
    ver(28, 24, 815, 0x1145d, [(6, 28), (11, 28), (16, 28), (19, 28)]),
    ver(28, 26, 901, 0x12a17, [(6, 30), (13, 26), (18, 28), (21, 28)]),
    ver(28, 28, 991, 0x13532, [(7, 28), (14, 26), (21, 26), (25, 26)]),
    ver(32, 28, 1085, 0x149a6, [(8, 28), (16, 26), (20, 30), (25, 28)]),
    ver(26, 22, 1156, 0x15683, [(8, 28), (17, 26), (23, 28), (25, 30)]),
    ver(24, 24, 1258, 0x168c9, [(9, 28), (17, 28), (23, 30), (34, 24)]),
    ver(28, 24, 1364, 0x177ec, [(9, 30), (18, 28), (25, 30), (30, 30)]),
    ver(26, 26, 1474, 0x18ec4, [(10, 30), (20, 28), (27, 30), (32, 30)]),
    ver(30, 26, 1588, 0x191e1, [(12, 26), (21, 28), (29, 30), (35, 30)]),
    ver(28, 28, 1706, 0x1afab, [(12, 28), (23, 28), (34, 28), (37, 30)]),
    ver(32, 28, 1828, 0x1b08e, [(12, 30), (25, 28), (34, 30), (40, 30)]),
    ver(24, 24, 1921, 0x1cc1a, [(13, 30), (26, 28), (35, 30), (42, 30)]),
    ver(28, 24, 2051, 0x1d33f, [(14, 30), (28, 28), (38, 30), (45, 30)]),
    ver(24, 26, 2185, 0x1ed75, [(15, 30), (29, 28), (40, 30), (48, 30)]),
    ver(28, 26, 2323, 0x1f250, [(16, 30), (31, 28), (43, 30), (51, 30)]),
    ver(32, 26, 2465, 0x209d5, [(17, 30), (33, 28), (45, 30), (54, 30)]),
    ver(28, 28, 2611, 0x216f0, [(18, 30), (35, 28), (48, 30), (57, 30)]),
    ver(32, 28, 2761, 0x228ba, [(19, 30), (37, 28), (51, 30), (60, 30)]),
    ver(28, 24, 2876, 0x2379f, [(19, 30), (38, 28), (53, 30), (63, 30)]),
    ver(22, 26, 3034, 0x24b0b, [(20, 30), (40, 28), (56, 30), (66, 30)]),
    ver(26, 26, 3196, 0x2542e, [(21, 30), (43, 28), (59, 30), (70, 30)]),
    ver(30, 26, 3362, 0x26a64, [(22, 30), (45, 28), (62, 30), (74, 30)]),
    ver(24, 28, 3532, 0x27541, [(24, 30), (47, 28), (65, 30), (77, 30)]),
    ver(28, 28, 3706, 0x28c69, [(25, 30), (49, 28), (68, 30), (81, 30)]),
];

/// The data bytes version v holds at level l (DataBytes).
fn data_bytes(v: usize, l: Level) -> usize {
    let (nblock, check) = VTAB[v].level[l as usize];
    VTAB[v].bytes - nblock * check
}

// A module of the plan: its role, whether it's dark, and for data and
// check modules which bit (Pixel).
type Pixel = u32;
const BLACK: Pixel = 1;
const INVERT: Pixel = 2;
const POSITION: u32 = 1;
const ALIGNMENT: u32 = 2;
const TIMING: u32 = 3;
const FORMAT: u32 = 4;
const PVERSION: u32 = 5;
const UNUSED: u32 = 6;
const DATA: u32 = 7;
const CHECK: u32 = 8;
const EXTRA: u32 = 9;

const fn rp(r: u32) -> Pixel {
    r << 2
}

fn role(p: Pixel) -> u32 {
    (p >> 2) & 15
}

const fn off(o: usize) -> Pixel {
    (o as u32) << 6
}

struct Plan {
    pixel: Vec<Vec<Pixel>>,
}

impl Plan {
    /// Version v at level l, mask 0 (NewPlan).
    fn new(v: usize, l: Level) -> Plan {
        let mut p = Plan::version(v);
        p.format(l);
        p.levels(v, l);
        p.mask();
        p
    }

    /// The fixed patterns (vplan), indexed as Go's loops index them.
    #[allow(clippy::needless_range_loop)]
    fn version(v: usize) -> Plan {
        let siz = 17 + v * 4;
        let mut m = vec![vec![0 as Pixel; siz]; siz];
        const TI: usize = 6;
        for i in 0..siz {
            let mut p = rp(TIMING);
            if i & 1 == 0 {
                p |= BLACK;
            }
            m[i][TI] = p;
            m[TI][i] = p;
        }
        pos_box(&mut m, 0, 0);
        pos_box(&mut m, siz - 7, 0);
        pos_box(&mut m, 0, siz - 7);
        let info = &VTAB[v];
        let mut x = 4;
        while x + 5 < siz {
            let mut y = 4;
            while y + 5 < siz {
                let skip =
                    (x < 7 && y < 7) || (x < 7 && y + 5 >= siz - 7) || (x + 5 >= siz - 7 && y < 7);
                if !skip {
                    align_box(&mut m, x, y);
                }
                y = if y == 4 { info.apos } else { y + info.astride };
            }
            x = if x == 4 { info.apos } else { x + info.astride };
        }
        let mut pat = info.pattern;
        if pat != 0 {
            for x in 0..6 {
                for y in 0..3 {
                    let mut p = rp(PVERSION);
                    if pat & 1 != 0 {
                        p |= BLACK;
                    }
                    m[siz - 11 + y][x] = p;
                    m[x][siz - 11 + y] = p;
                    pat >>= 1;
                }
            }
        }
        m[siz - 8][8] = rp(UNUSED) | BLACK;
        Plan { pixel: m }
    }

    /// The format modules, for level l and mask 0 (fplan).
    fn format(&mut self, l: Level) {
        let mut fb = ((l as u32) ^ 1) << 13;
        const FORMAT_POLY: u32 = 0x537;
        let mut rem = fb;
        for i in (10..=14).rev() {
            if rem & (1 << i) != 0 {
                rem ^= FORMAT_POLY << (i - 10);
            }
        }
        fb |= rem;
        let invert: u32 = 0x5412;
        let siz = self.pixel.len();
        let m = &mut self.pixel;
        for i in 0..15usize {
            let mut pix = rp(FORMAT) + off(i);
            if (fb >> i) & 1 == 1 {
                pix |= BLACK;
            }
            if (invert >> i) & 1 == 1 {
                pix ^= INVERT | BLACK;
            }
            match i {
                0..6 => m[i][8] = pix,
                6..8 => m[i + 1][8] = pix,
                8 => m[8][7] = pix,
                _ => m[8][14 - i] = pix,
            }
            if i < 8 {
                m[8][siz - 1 - i] = pix;
            } else {
                m[siz - 1 - (14 - i)][8] = pix;
            }
        }
    }

    /// Where each data and check bit goes (lplan).
    fn levels(&mut self, v: usize, l: Level) {
        let (nblock, ne) = VTAB[v].level[l as usize];
        let nde = (VTAB[v].bytes - ne * nblock) / nblock;
        let extra = (VTAB[v].bytes - ne * nblock) % nblock;
        let data_bits = (nde * nblock + extra) * 8;
        let check_bits = ne * nblock * 8;
        let mut data: Vec<Pixel> = (0..data_bits).map(|i| rp(DATA) | off(i)).collect();
        let mut check: Vec<Pixel> = (0..check_bits)
            .map(|i| rp(CHECK) | off(i + data_bits))
            .collect();
        let mut data_list = Vec::with_capacity(nblock);
        let mut check_list = Vec::with_capacity(nblock);
        for i in 0..nblock {
            let nd = if i >= nblock - extra { nde + 1 } else { nde };
            let rest = data.split_off(nd * 8);
            data_list.push(std::mem::replace(&mut data, rest));
            let rest = check.split_off(ne * 8);
            check_list.push(std::mem::replace(&mut check, rest));
        }
        let mut bits: Vec<Pixel> = Vec::with_capacity(data_bits + check_bits);
        for i in 0..nde + 1 {
            for b in &data_list {
                if i * 8 < b.len() {
                    bits.extend_from_slice(&b[i * 8..(i + 1) * 8]);
                }
            }
        }
        for i in 0..ne {
            for b in &check_list {
                if i * 8 < b.len() {
                    bits.extend_from_slice(&b[i * 8..(i + 1) * 8]);
                }
            }
        }
        bits.extend(std::iter::repeat_n(rp(EXTRA), 7));
        let siz = self.pixel.len();
        let m = &mut self.pixel;
        let mut src = bits.into_iter();
        let mut put = |m: &mut Vec<Vec<Pixel>>, y: usize, x: usize| {
            if role(m[y][x]) == 0 {
                m[y][x] = src.next().unwrap_or(rp(EXTRA));
            }
        };
        let mut x = siz;
        while x > 0 {
            for y in (0..siz).rev() {
                put(m, y, x - 1);
                put(m, y, x - 2);
            }
            x -= 2;
            if x == 7 {
                x -= 1;
            }
            for y in 0..siz {
                put(m, y, x - 1);
                put(m, y, x - 2);
            }
            x -= 2;
        }
    }

    /// Mask 0 over the data (mplan).
    fn mask(&mut self) {
        for (y, row) in self.pixel.iter_mut().enumerate() {
            for (x, pix) in row.iter_mut().enumerate() {
                let r = role(*pix);
                if (r == DATA || r == CHECK || r == EXTRA) && (y + x) % 2 == 0 {
                    *pix ^= BLACK | INVERT;
                }
            }
        }
    }
}

/// A finder box at x, y with its light border (posBox).
fn pos_box(m: &mut [Vec<Pixel>], x: usize, y: usize) {
    let pos = rp(POSITION);
    for dy in 0..7 {
        for dx in 0..7 {
            let mut p = pos;
            if dx == 0
                || dx == 6
                || dy == 0
                || dy == 6
                || (2..=4).contains(&dx) && (2..=4).contains(&dy)
            {
                p |= BLACK;
            }
            m[y + dy][x + dx] = p;
        }
    }
    let n = m.len() as isize;
    let (xi, yi) = (x as isize, y as isize);
    for dy in -1..8isize {
        let yy = yi + dy;
        if (0..n).contains(&yy) {
            if xi > 0 {
                m[yy as usize][x - 1] = pos;
            }
            if xi + 7 < n {
                m[yy as usize][x + 7] = pos;
            }
        }
    }
    for dx in -1..8isize {
        let xx = xi + dx;
        if (0..n).contains(&xx) {
            if yi > 0 {
                m[y - 1][xx as usize] = pos;
            }
            if yi + 7 < n {
                m[y + 7][xx as usize] = pos;
            }
        }
    }
}

/// An alignment box at x, y (alignBox).
fn align_box(m: &mut [Vec<Pixel>], x: usize, y: usize) {
    let align = rp(ALIGNMENT);
    for dy in 0..5 {
        for dx in 0..5 {
            let mut p = align;
            if dx == 0 || dx == 4 || dy == 0 || dy == 4 || dx == 2 && dy == 2 {
                p |= BLACK;
            }
            m[y + dy][x + dx] = p;
        }
    }
}

/// GF(256) over 0x11d, generator 2 (Field): exp and log tables.
struct Field {
    log: [u8; 256],
    exp: [u8; 510],
}

fn field() -> &'static Field {
    static F: std::sync::OnceLock<Field> = std::sync::OnceLock::new();
    F.get_or_init(|| {
        let mut f = Field {
            log: [0; 256],
            exp: [0; 510],
        };
        let mut x: u32 = 1;
        for i in 0..255 {
            f.exp[i] = x as u8;
            f.exp[i + 255] = x as u8;
            f.log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= 0x11d;
            }
        }
        f.log[0] = 255;
        f
    })
}

impl Field {
    fn exp_of(&self, e: usize) -> u8 {
        self.exp[e % 255]
    }

    fn mul(&self, x: u8, y: u8) -> u8 {
        if x == 0 || y == 0 {
            return 0;
        }
        self.exp[self.log[x as usize] as usize + self.log[y as usize] as usize]
    }
}

/// Reed-Solomon with c check bytes (RSEncoder).
struct Rs {
    c: usize,
    lgen: Vec<u8>,
}

impl Rs {
    fn new(c: usize) -> Rs {
        let f = field();
        let mut p = vec![0u8; c + 1];
        p[c] = 1;
        for i in 0..c {
            let k = f.exp_of(i);
            for j in 0..c {
                p[j] = f.mul(p[j], k) ^ p[j + 1];
            }
            p[c] = f.mul(p[c], k);
        }
        let lgen = p
            .iter()
            .map(|&v| if v == 0 { 255 } else { f.log[v as usize] })
            .collect();
        Rs { c, lgen }
    }

    /// data's check bytes (ECC).
    fn ecc(&self, data: &[u8]) -> Vec<u8> {
        let f = field();
        let mut p = data.to_vec();
        p.resize(data.len() + self.c, 0);
        let lgen = &self.lgen[1..];
        for i in 0..data.len() {
            let c = p[i];
            if c == 0 {
                continue;
            }
            let base = f.log[c as usize] as usize;
            for (j, &lg) in lgen.iter().enumerate() {
                if lg != 255 {
                    p[i + 1 + j] ^= f.exp[base + lg as usize];
                }
            }
        }
        p.split_off(data.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A code as rows of # and ., for comparing with Go's.
    fn art(c: &Code) -> String {
        let n = c.size as isize;
        (0..n)
            .map(|y| {
                (0..n)
                    .map(|x| if c.black(x, y) { '#' } else { '.' })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Codes as Go's rsc.io/qr draws them: their size, and a hash of their
    /// modules taken from Go's output for the same text and level.
    #[test]
    fn as_go_draws_them() {
        let cases: &[(&str, Level, usize, u64)] = &[
            ("1234567", Level::H, 21, 13383338646524956783),
            ("HELLO WORLD", Level::Q, 21, 6505038745940797579),
            (
                "https://rush.example.ts.net:8443/pair#ABCD2345",
                Level::H,
                41,
                82862866676074921,
            ),
            (
                "https://rush.example.ts.net:8443/pair#ABCD2345",
                Level::Q,
                33,
                14290216583573021861,
            ),
        ];
        for &(text, l, size, want) in cases {
            let c = encode(text, l).expect("encodes");
            assert_eq!(c.size, size, "{text} {l:?}");
            assert_eq!(fnv(&art(&c)), want, "{text} {l:?}:\n{}", art(&c));
        }
    }

    fn fnv(s: &str) -> u64 {
        s.bytes().fold(0xcbf29ce484222325, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        })
    }

    #[test]
    fn too_long() {
        assert!(encode(&"x".repeat(4000), Level::H).is_err());
    }
}
