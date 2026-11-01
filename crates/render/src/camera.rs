//! Column-major 4x4 matrices in the layout WGSL `mat4x4f` reads from a uniform buffer.
//! Right-handed, with clip depth in [0, 1] as wgpu expects.

pub type Mat4 = [f32; 16];

pub const IDENTITY: Mat4 = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// Right-handed view matrix looking from `eye` at `target`.
pub fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Mat4 {
    let f = normalize(sub(target, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        s[0],
        u[0],
        -f[0],
        0.0,
        s[1],
        u[1],
        -f[1],
        0.0,
        s[2],
        u[2],
        -f[2],
        0.0,
        -dot(s, eye),
        -dot(u, eye),
        dot(f, eye),
        1.0,
    ]
}

/// Right-handed perspective projection with depth mapped to [0, 1].
/// `fovy` is the vertical field of view in radians.
pub fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fovy * 0.5).tan();
    [
        f / aspect,
        0.0,
        0.0,
        0.0,
        0.0,
        f,
        0.0,
        0.0,
        0.0,
        0.0,
        far / (near - far),
        -1.0,
        0.0,
        0.0,
        far * near / (near - far),
        0.0,
    ]
}

/// Rotation about the X axis by `angle` radians.
pub fn rotation_x(angle: f32) -> Mat4 {
    let (s, c) = angle.sin_cos();
    [
        1.0, 0.0, 0.0, 0.0, 0.0, c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

/// Column-major product `a * b`.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

/// Inverse of a column-major matrix by cofactor expansion. `None` when the matrix is singular.
pub fn inverse(m: &Mat4) -> Option<Mat4> {
    let a = |i: usize| m[i];
    let mut inv = [0.0f32; 16];
    inv[0] = a(5) * a(10) * a(15) - a(5) * a(11) * a(14) - a(9) * a(6) * a(15)
        + a(9) * a(7) * a(14)
        + a(13) * a(6) * a(11)
        - a(13) * a(7) * a(10);
    inv[4] = -a(4) * a(10) * a(15) + a(4) * a(11) * a(14) + a(8) * a(6) * a(15)
        - a(8) * a(7) * a(14)
        - a(12) * a(6) * a(11)
        + a(12) * a(7) * a(10);
    inv[8] = a(4) * a(9) * a(15) - a(4) * a(11) * a(13) - a(8) * a(5) * a(15)
        + a(8) * a(7) * a(13)
        + a(12) * a(5) * a(11)
        - a(12) * a(7) * a(9);
    inv[12] = -a(4) * a(9) * a(14) + a(4) * a(10) * a(13) + a(8) * a(5) * a(14)
        - a(8) * a(6) * a(13)
        - a(12) * a(5) * a(10)
        + a(12) * a(6) * a(9);
    inv[1] = -a(1) * a(10) * a(15) + a(1) * a(11) * a(14) + a(9) * a(2) * a(15)
        - a(9) * a(3) * a(14)
        - a(13) * a(2) * a(11)
        + a(13) * a(3) * a(10);
    inv[5] = a(0) * a(10) * a(15) - a(0) * a(11) * a(14) - a(8) * a(2) * a(15)
        + a(8) * a(3) * a(14)
        + a(12) * a(2) * a(11)
        - a(12) * a(3) * a(10);
    inv[9] = -a(0) * a(9) * a(15) + a(0) * a(11) * a(13) + a(8) * a(1) * a(15)
        - a(8) * a(3) * a(13)
        - a(12) * a(1) * a(11)
        + a(12) * a(3) * a(9);
    inv[13] = a(0) * a(9) * a(14) - a(0) * a(10) * a(13) - a(8) * a(1) * a(14)
        + a(8) * a(2) * a(13)
        + a(12) * a(1) * a(10)
        - a(12) * a(2) * a(9);
    inv[2] = a(1) * a(6) * a(15) - a(1) * a(7) * a(14) - a(5) * a(2) * a(15)
        + a(5) * a(3) * a(14)
        + a(13) * a(2) * a(7)
        - a(13) * a(3) * a(6);
    inv[6] = -a(0) * a(6) * a(15) + a(0) * a(7) * a(14) + a(4) * a(2) * a(15)
        - a(4) * a(3) * a(14)
        - a(12) * a(2) * a(7)
        + a(12) * a(3) * a(6);
    inv[10] = a(0) * a(5) * a(15) - a(0) * a(7) * a(13) - a(4) * a(1) * a(15)
        + a(4) * a(3) * a(13)
        + a(12) * a(1) * a(7)
        - a(12) * a(3) * a(5);
    inv[14] = -a(0) * a(5) * a(14) + a(0) * a(6) * a(13) + a(4) * a(1) * a(14)
        - a(4) * a(2) * a(13)
        - a(12) * a(1) * a(6)
        + a(12) * a(2) * a(5);
    inv[3] = -a(1) * a(6) * a(11) + a(1) * a(7) * a(10) + a(5) * a(2) * a(11)
        - a(5) * a(3) * a(10)
        - a(9) * a(2) * a(7)
        + a(9) * a(3) * a(6);
    inv[7] = a(0) * a(6) * a(11) - a(0) * a(7) * a(10) - a(4) * a(2) * a(11)
        + a(4) * a(3) * a(10)
        + a(8) * a(2) * a(7)
        - a(8) * a(3) * a(6);
    inv[11] = -a(0) * a(5) * a(11) + a(0) * a(7) * a(9) + a(4) * a(1) * a(11)
        - a(4) * a(3) * a(9)
        - a(8) * a(1) * a(7)
        + a(8) * a(3) * a(5);
    inv[15] = a(0) * a(5) * a(10) - a(0) * a(6) * a(9) - a(4) * a(1) * a(10)
        + a(4) * a(2) * a(9)
        + a(8) * a(1) * a(6)
        - a(8) * a(2) * a(5);
    let det = a(0) * inv[0] + a(1) * inv[4] + a(2) * inv[8] + a(3) * inv[12];
    if det.abs() < 1e-12 {
        return None;
    }
    let scale = 1.0 / det;
    Some(inv.map(|v| v * scale))
}

/// Rotation matrix for a unit quaternion `[x, y, z, w]`, with translation zero.
pub fn quat_to_mat4(q: [f32; 4]) -> Mat4 {
    let [x, y, z, w] = q;
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    [
        1.0 - 2.0 * (yy + zz),
        2.0 * (xy + wz),
        2.0 * (xz - wy),
        0.0,
        2.0 * (xy - wz),
        1.0 - 2.0 * (xx + zz),
        2.0 * (yz + wx),
        0.0,
        2.0 * (xz + wy),
        2.0 * (yz - wx),
        1.0 - 2.0 * (xx + yy),
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

/// Translation matrix.
/// A uniform scale.
pub fn scale(s: f32) -> Mat4 {
    [
        s, 0.0, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

pub fn translate(t: [f32; 3]) -> Mat4 {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, t[0], t[1], t[2], 1.0,
    ]
}

/// Applies a column-major matrix to a point (w = 1), returning the xyz part.
pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let x = m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12];
    let y = m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13];
    let z = m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14];
    [x, y, z]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    [v[0] / len, v[1] / len, v[2] / len]
}
