// Voxel raymarcher: one thread per pixel, three-level DDA (Amanatides & Woo):
// chunks (64³) → bricks (8³) → voxels.
//
// Data layout (mirrors mv_render::gpu_volume):
//   chunk_grid[c]                     entry for chunk cell c of the window
//   chunk_bricks[slot * 512 + b]      entry for brick b of the chunk at `slot`
//   brick_pool[brick * 256 + i / 2]   two u16 material ids per u32 (low half = even i)
//   materials[id]                     linear colour + alpha, emissive, liquid flag
// Grid and table entries: EMPTY (air), UNIFORM_BIT | material (whole chunk/brick is
// one material), or a slot index into the next level.
// Index orders are always x-fastest: x + y*N + z*N*N.
// Coordinates are in voxels, relative to the volume's minimum corner.
//
// Rigid bodies (mirrors mv_render::bodies): body_headers[i] has the inverse rotation
// (rows), the position and the size + voxel offset of a dense grid stored in
// body_voxels (two u16 per u32). They are traced after the world; the nearest hit
// wins.
//
// Lighting (milestone 7, ADR 010): sun shadows (a second ray towards the sun),
// ambient occlusion at two scales (per-corner occupancy of the hit face, plus short
// rays that also gather light from emissive voxels), self-lit emissives, one layer
// of transparency (water, steam, glass...) with absorption by thickness and a
// Fresnel sky reflection on liquids, and fog tinted towards the sun.

const EMPTY: u32 = 0xffffffffu;
const UNIFORM_BIT: u32 = 0x80000000u;
const BRICK: i32 = 8;
const BRICKS_PER_CHUNK_AXIS: i32 = 8;
const CHUNK: i32 = 64;
const MAX_CHUNK_STEPS: u32 = 512u;
const FAR: f32 = 1e30;
/// Materials with alpha below this are translucent.
const OPAQUE_ALPHA: f32 = 0.98;

struct Frame {
    camera_pos: vec4<f32>,  // xyz: eye position relative to the volume origin
    forward: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    // x: tan(fov_y / 2), y: aspect ratio, z: width, w: height
    projection: vec4<f32>,
    sun_dir: vec4<f32>,     // xyz: direction towards the sun (normalised)
    brush: vec4<f32>,       // xyz: brush centre (volume-relative), w: radius (<= 0: none)
    fog: vec4<f32>,         // x: k, y: start s in 1 - exp(-((t - s) * k)^2), z: ambient °C
    volume_dims: vec4<u32>, // xyz: volume size in chunks, w: flags (bit 0: heat view)
    bodies: vec4<u32>,      // x: number of rigid bodies
    // x: shadow distance (0 = off), y: AO strength (0 = off), z: normal smoothing
    // (0 = flat faces), w: per-voxel colour noise (1 = full)
    lighting: vec4<f32>,
};

struct Material {
    color: vec4<f32>,  // linear rgb + alpha
    params: vec4<f32>, // x: emissive, y: 1 for liquids (Fresnel reflection)
};

struct Hit {
    found: bool,
    t: f32,
    material: u32,
    axis: i32,        // axis of the face that was hit (0 = x, 1 = y, 2 = z)
    voxel: vec3<i32>,
};

struct Body {
    row0: vec4<f32>,      // inverse rotation (world -> body), by rows
    row1: vec4<f32>,
    row2: vec4<f32>,
    position: vec4<f32>,  // xyz: grid frame origin, volume-relative
    size: vec4<u32>,      // xyz: grid size; w: offset of the first voxel (u16 units)
};

struct BodyHit {
    found: bool,
    t: f32,
    material: u32,
    normal: vec3<f32>,    // world space
    seed: vec3<i32>,      // for the per-voxel colour variation
};

/// What a ray found, world or body, ready to shade.
struct Surface {
    found: bool,
    t: f32,
    material: u32,
    normal: vec3<f32>,
    voxel: vec3<i32>,     // world voxel (meaningless for bodies)
    seed: vec3<i32>,
    is_body: bool,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> chunk_grid: array<u32>;
@group(0) @binding(2) var<storage, read> chunk_bricks: array<u32>;
@group(0) @binding(3) var<storage, read> brick_pool: array<u32>;
@group(0) @binding(4) var<storage, read> materials: array<Material>;
@group(0) @binding(5) var output: texture_storage_2d<rgba16float, write>;
// Heat view: chunk cell of the window -> field slot (or EMPTY = ambient); fields of
// 16³ temperature cells (4³ voxels each), x-fastest.
@group(0) @binding(6) var<storage, read> heat_index: array<u32>;
@group(0) @binding(7) var<storage, read> heat_pool: array<f32>;
@group(0) @binding(8) var<storage, read> body_headers: array<Body>;
@group(0) @binding(9) var<storage, read> body_voxels: array<u32>;

fn no_hit() -> Hit {
    return Hit(false, 0.0, 0u, 0, vec3<i32>(0));
}

// Raw 16 bits of a voxel: material in the low 12, fill level in the high 4 (0 =
// full, else sixteenths of the voxel; partly filled liquids, ADR 014).
fn voxel_raw(brick: u32, v: vec3<i32>) -> u32 {
    let i = u32(v.x + v.y * 8 + v.z * 64);
    let word = brick_pool[brick * 256u + (i >> 1u)];
    return (word >> ((i & 1u) * 16u)) & 0xffffu;
}

fn voxel_material(brick: u32, v: vec3<i32>) -> u32 {
    return voxel_raw(brick, v) & 0x0fffu;
}

fn min_axis(v: vec3<f32>) -> i32 {
    if v.x < v.y && v.x < v.z {
        return 0;
    }
    if v.y < v.z {
        return 1;
    }
    return 2;
}

// Entry of the 8³ brick holding voxel `v` (volume-relative): EMPTY (air, also
// outside the window), `UNIFORM_BIT | material`, or a brick pool slot.
fn brick_at(v: vec3<i32>) -> u32 {
    let dims = vec3<i32>(frame.volume_dims.xyz);
    if any(v < vec3<i32>(0)) || any(v >= dims * CHUNK) {
        return EMPTY;
    }
    let c = v / CHUNK;
    let entry = chunk_grid[c.x + c.y * dims.x + c.z * dims.x * dims.y];
    if entry == EMPTY || (entry & UNIFORM_BIT) != 0u {
        return entry;
    }
    let b = (v - c * CHUNK) / BRICK;
    return chunk_bricks[entry * 512u + u32(b.x + b.y * 8 + b.z * 64)];
}

// Material of voxel `l` (0..7 per axis) of a brick entry from `brick_at`.
fn brick_voxel(be: u32, l: vec3<i32>) -> u32 {
    if be == EMPTY {
        return 0u;
    }
    if (be & UNIFORM_BIT) != 0u {
        return be & 0xffffu;
    }
    return voxel_material(be, l);
}

// Raw 16 bits (material and fill level) of voxel `v`; 0 (air) outside the window.
fn voxel_raw_at(v: vec3<i32>) -> u32 {
    let be = brick_at(v);
    if be == EMPTY {
        return 0u;
    }
    if (be & UNIFORM_BIT) != 0u {
        return be & 0x0fffu;
    }
    return voxel_raw(be, v & vec3<i32>(BRICK - 1));
}

// Fraction of the voxel a raw value fills (1 unless a partly filled liquid).
fn raw_fill(raw: u32) -> f32 {
    let level = raw >> 12u;
    return select(f32(level) / 16.0, 1.0, level == 0u);
}

// Height of the surface of liquid `m` in the column of `v`, relative to `v.y`:
// `v`'s fill (more if the voxel above is liquid too), or below it if `v` is empty
// over liquid; `own` where there is no liquid (walls keep the surface flat).
fn liquid_height(v: vec3<i32>, m: u32, own: f32) -> f32 {
    let raw = voxel_raw_at(v);
    let here = raw & 0x0fffu;
    if here == m {
        let above = voxel_raw_at(v + vec3<i32>(0, 1, 0));
        if (above & 0x0fffu) == m {
            return 1.0 + raw_fill(above);
        }
        return raw_fill(raw);
    }
    if here == 0u {
        let below = voxel_raw_at(v - vec3<i32>(0, 1, 0));
        if (below & 0x0fffu) == m {
            return raw_fill(below) - 1.0;
        }
    }
    return own;
}

// Normal of the top surface of liquid `m` at voxel `v`, from the surface heights
// of its neighbours: the water surface reads as one smooth sheet.
fn liquid_normal(v: vec3<i32>, m: u32) -> vec3<f32> {
    let own = raw_fill(voxel_raw_at(v));
    let dx = liquid_height(v + vec3<i32>(1, 0, 0), m, own) - liquid_height(v - vec3<i32>(1, 0, 0), m, own);
    let dz = liquid_height(v + vec3<i32>(0, 0, 1), m, own) - liquid_height(v - vec3<i32>(0, 0, 1), m, own);
    return normalize(vec3<f32>(-0.5 * dx, 1.0, -0.5 * dz));
}

// Material at voxel `v` (volume-relative); air outside the window.
fn voxel_at(v: vec3<i32>) -> u32 {
    return brick_voxel(brick_at(v), v & vec3<i32>(BRICK - 1));
}

fn is_opaque(m: u32) -> bool {
    return m != 0u && materials[m].color.a >= OPAQUE_ALPHA;
}

// Voxel-level DDA inside one 8³ brick, starting at ray parameter t_start. Voxels of
// material `skip` count as empty.
fn trace_brick(ro: vec3<f32>, rd: vec3<f32>, inv: vec3<f32>, step: vec3<i32>,
               t_start: f32, bmin: vec3<i32>, brick: u32, entry_axis: i32, skip: u32) -> Hit {
    let bmax = bmin + vec3<i32>(BRICK - 1);
    let p = ro + rd * t_start;
    var v = clamp(vec3<i32>(floor(p)), bmin, bmax);
    let delta = abs(inv);
    var t_max = (vec3<f32>(v) + select(vec3<f32>(0.0), vec3<f32>(1.0), rd > vec3<f32>(0.0)) - ro) * inv;
    var axis = entry_axis;
    var t = t_start;
    for (var i = 0; i < 3 * BRICK; i++) {
        let raw = voxel_raw(brick, v - bmin);
        let m = raw & 0x0fffu;
        if m != 0u && m != skip {
            let level = raw >> 12u;
            if level == 0u {
                return Hit(true, t, m, axis, v);
            }
            // Partly filled liquid: it fills the voxel up to `top`. Entering below
            // it is a hit; from above, the ray hits the surface if it goes down
            // through it before leaving the voxel.
            let top = f32(v.y) + f32(level) / 16.0;
            if ro.y + rd.y * t <= top + 1e-4 {
                return Hit(true, t, m, axis, v);
            }
            let t_out = min(t_max.x, min(t_max.y, t_max.z));
            if rd.y < 0.0 {
                let ts = (top - ro.y) * inv.y;
                if ts <= t_out {
                    return Hit(true, ts, m, 1, v);
                }
            }
        }
        axis = min_axis(t_max);
        t = t_max[axis];
        t_max[axis] += delta[axis];
        v[axis] += step[axis];
        if v[axis] < bmin[axis] || v[axis] > bmax[axis] {
            break;
        }
    }
    return no_hit();
}

// Brick-level DDA inside one non-uniform chunk, starting at ray parameter t_start.
// `cmin` is the chunk's minimum corner in voxels, `slot` its brick table.
fn trace_chunk(ro: vec3<f32>, rd: vec3<f32>, inv: vec3<f32>, step: vec3<i32>,
               t_start: f32, t_exit: f32, cmin: vec3<i32>, slot: u32, entry_axis: i32,
               skip: u32) -> Hit {
    let gmin = cmin / BRICK;
    let p = ro + rd * t_start;
    var b = clamp(vec3<i32>(floor(p / f32(BRICK))) - gmin, vec3<i32>(0),
                  vec3<i32>(BRICKS_PER_CHUNK_AXIS - 1));
    let delta = abs(f32(BRICK) * inv);
    var t_max = ((vec3<f32>(gmin + b) + select(vec3<f32>(0.0), vec3<f32>(1.0), rd > vec3<f32>(0.0)))
                 * f32(BRICK) - ro) * inv;
    var axis = entry_axis;
    var t = t_start;
    // A ray crosses at most 3 * 8 - 2 bricks of a chunk.
    for (var i = 0; i < 3 * BRICKS_PER_CHUNK_AXIS; i++) {
        let entry = chunk_bricks[slot * 512u + u32(b.x + b.y * 8 + b.z * 64)];
        if entry != EMPTY {
            let bmin = (gmin + b) * BRICK;
            if (entry & UNIFORM_BIT) != 0u {
                if (entry & 0xffffu) != skip {
                    // Solid uniform brick: the entry point is the hit.
                    let v = clamp(vec3<i32>(floor(ro + rd * t)), bmin, bmin + vec3<i32>(BRICK - 1));
                    return Hit(true, t, entry & 0xffffu, axis, v);
                }
            } else {
                let hit = trace_brick(ro, rd, inv, step, t, bmin, entry, axis, skip);
                if hit.found {
                    return hit;
                }
            }
        }
        axis = min_axis(t_max);
        t = t_max[axis];
        if t > t_exit {
            break;
        }
        t_max[axis] += delta[axis];
        b[axis] += step[axis];
        if b[axis] < 0 || b[axis] >= BRICKS_PER_CHUNK_AXIS {
            break;
        }
    }
    return no_hit();
}

// Chunk-level DDA over the window: empty chunks are skipped in one step, uniform
// chunks hit immediately, the rest descend to bricks and then voxels. Voxels of
// `skip` count as empty; the search stops beyond `t_limit`.
fn trace(ro: vec3<f32>, rd_in: vec3<f32>, skip: u32, t_limit: f32) -> Hit {
    // Avoid divisions by zero for axis-aligned rays.
    let eps = 1e-8;
    let rd = select(rd_in, vec3<f32>(eps), abs(rd_in) < vec3<f32>(eps));
    let inv = 1.0 / rd;

    let dims = vec3<i32>(frame.volume_dims.xyz);
    let bounds = vec3<f32>(dims * CHUNK);
    let t0 = -ro * inv;
    let t1 = (bounds - ro) * inv;
    let t_near = min(t0, t1);
    let t_far = max(t0, t1);
    let t_enter = max(max(t_near.x, t_near.y), max(t_near.z, 0.0));
    let t_exit = min(min(min(t_far.x, t_far.y), t_far.z), t_limit);
    if t_enter >= t_exit {
        return no_hit();
    }

    // Face through which the ray enters the volume (only meaningful if outside).
    var axis = 0;
    if t_near.y > t_near.x && t_near.y >= t_near.z {
        axis = 1;
    } else if t_near.z > t_near.x && t_near.z > t_near.y {
        axis = 2;
    }

    let step = vec3<i32>(sign(rd));
    let p = ro + rd * t_enter;
    var c = clamp(vec3<i32>(floor(p / f32(CHUNK))), vec3<i32>(0), dims - 1);
    let delta = abs(f32(CHUNK) * inv);
    var t_max = ((vec3<f32>(c) + select(vec3<f32>(0.0), vec3<f32>(1.0), rd > vec3<f32>(0.0)))
                 * f32(CHUNK) - ro) * inv;
    var t = t_enter;

    for (var i = 0u; i < MAX_CHUNK_STEPS; i++) {
        let entry = chunk_grid[c.x + c.y * dims.x + c.z * dims.x * dims.y];
        if entry != EMPTY {
            let cmin = c * CHUNK;
            if (entry & UNIFORM_BIT) != 0u {
                if (entry & 0xffffu) != skip {
                    let v = clamp(vec3<i32>(floor(ro + rd * t)), cmin, cmin + vec3<i32>(CHUNK - 1));
                    return Hit(true, t, entry & 0xffffu, axis, v);
                }
            } else {
                let hit = trace_chunk(ro, rd, inv, step, t, t_exit, cmin, entry, axis, skip);
                if hit.found {
                    return hit;
                }
            }
        }
        axis = min_axis(t_max);
        t = t_max[axis];
        if t > t_exit {
            break;
        }
        t_max[axis] += delta[axis];
        c[axis] += step[axis];
        if c[axis] < 0 || c[axis] >= dims[axis] {
            break;
        }
    }
    return no_hit();
}

fn body_voxel(b: Body, c: vec3<i32>) -> u32 {
    let s = vec3<i32>(b.size.xyz);
    let i = b.size.w + u32(c.x + c.y * s.x + c.z * s.x * s.y);
    let word = body_voxels[i >> 1u];
    return (word >> ((i & 1u) * 16u)) & 0xffffu;
}

// Nearest rigid body voxel along the ray closer than t_best.
fn trace_bodies(ro: vec3<f32>, rd: vec3<f32>, t_best: f32) -> BodyHit {
    var best = BodyHit(false, t_best, 0u, vec3<f32>(0.0), vec3<i32>(0));
    for (var i = 0u; i < frame.bodies.x; i++) {
        let b = body_headers[i];
        let rel = ro - b.position.xyz;
        let o = vec3<f32>(dot(b.row0.xyz, rel), dot(b.row1.xyz, rel), dot(b.row2.xyz, rel));
        let d = vec3<f32>(dot(b.row0.xyz, rd), dot(b.row1.xyz, rd), dot(b.row2.xyz, rd));
        let size = vec3<f32>(b.size.xyz);
        // Slab test against the grid box.
        let inv = 1.0 / select(d, vec3<f32>(1e-12), abs(d) < vec3<f32>(1e-12));
        let t0 = (vec3<f32>(0.0) - o) * inv;
        let t1 = (size - o) * inv;
        let tn = min(t0, t1);
        let tf = max(t0, t1);
        let t_enter = max(max(tn.x, tn.y), max(tn.z, 0.0));
        let t_exit = min(min(tf.x, tf.y), tf.z);
        if t_exit <= t_enter || t_enter >= best.t {
            continue;
        }
        // Voxel DDA inside the grid.
        let step = vec3<i32>(sign(d));
        let p = o + d * (t_enter + 1e-4);
        var c = clamp(vec3<i32>(floor(p)), vec3<i32>(0), vec3<i32>(b.size.xyz) - 1);
        let delta = abs(inv);
        var t_max = (vec3<f32>(c) + max(vec3<f32>(step), vec3<f32>(0.0)) - o) * inv;
        var axis = 0;
        if tn.y >= tn.x && tn.y >= tn.z { axis = 1; } else if tn.z >= tn.x { axis = 2; }
        var t = t_enter;
        let max_steps = b.size.x + b.size.y + b.size.z;
        for (var s = 0u; s < max_steps; s++) {
            if t >= best.t {
                break;
            }
            let m = body_voxel(b, c);
            if m != 0u {
                var n = vec3<f32>(0.0);
                n[axis] = -sign(d[axis]);
                // Back to world space: R * n, and R's columns are the rows above.
                let wn = n.x * vec3<f32>(b.row0.x, b.row1.x, b.row2.x)
                    + n.y * vec3<f32>(b.row0.y, b.row1.y, b.row2.y)
                    + n.z * vec3<f32>(b.row0.z, b.row1.z, b.row2.z);
                best = BodyHit(true, t, m, wn, c + vec3<i32>(i32(i) * 7919));
                break;
            }
            axis = min_axis(t_max);
            t = t_max[axis];
            t_max[axis] += delta[axis];
            c[axis] += step[axis];
            if c[axis] < 0 || c[axis] >= i32(b.size[axis]) {
                break;
            }
        }
    }
    return best;
}

// Nearest world or body surface along the ray (world voxels of `skip` ignored).
fn find_surface(ro: vec3<f32>, rd: vec3<f32>, skip: u32, t_limit: f32) -> Surface {
    let hit = trace(ro, rd, skip, t_limit);
    let body = trace_bodies(ro, rd, select(t_limit, hit.t, hit.found));
    if body.found {
        return Surface(true, body.t, body.material, body.normal, vec3<i32>(0), body.seed, true);
    }
    if hit.found {
        var n = vec3<f32>(0.0);
        n[hit.axis] = -sign(rd[hit.axis]);
        return Surface(true, hit.t, hit.material, n, hit.voxel, hit.voxel, false);
    }
    return Surface(false, FAR, 0u, vec3<f32>(0.0), vec3<i32>(0), vec3<i32>(0), false);
}

fn hash3(p: vec3<i32>) -> f32 {
    // WGSL forbids mixing bitwise and arithmetic operators without parentheses.
    var h = (u32(p.x) * 73856093u) ^ (u32(p.y) * 19349663u) ^ (u32(p.z) * 83492791u);
    h = (h ^ (h >> 13u)) * 1274126177u;
    return f32(h & 0xffffu) / 65535.0;
}

// Smooth value noise in 0..1: hashed lattice values blended trilinearly.
fn value_noise(p: vec3<f32>) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = p - floor(p);
    let u = f * f * (3.0 - 2.0 * f);
    let x00 = mix(hash3(i), hash3(i + vec3<i32>(1, 0, 0)), u.x);
    let x10 = mix(hash3(i + vec3<i32>(0, 1, 0)), hash3(i + vec3<i32>(1, 1, 0)), u.x);
    let x01 = mix(hash3(i + vec3<i32>(0, 0, 1)), hash3(i + vec3<i32>(1, 0, 1)), u.x);
    let x11 = mix(hash3(i + vec3<i32>(0, 1, 1)), hash3(i + vec3<i32>(1, 1, 1)), u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z);
}

// Does voxel material `m` count as part of the surface of `own` when smoothing
// normals? Opaque voxels always do, a translucent one only next to itself (water
// over stone has its own surface).
fn surface_solid(m: u32, own: u32) -> bool {
    return m != 0u && (m == own || is_opaque(m));
}

// Height of the surface at voxel `q` relative to the face being shaded (normal `n`):
// +1 if it rises there (the voxel in front of `q` is solid), -1 if it drops (`q`
// itself is empty), 0 if it continues flat.
fn relative_height(q: vec3<i32>, n: vec3<i32>, m: u32) -> f32 {
    if surface_solid(voxel_at(q + n), m) {
        return 1.0;
    }
    if !surface_solid(voxel_at(q), m) {
        return -1.0;
    }
    return 0.0;
}

// Shading normal of the face of world voxel `v` (material `m`) with normal `face`:
// the slope of the surface around it, measured from its height 1, 2 and 3 voxels
// away in the four directions along the face, tilts the face normal (by
// `frame.lighting.z`). The voxel staircase on slopes then shades like the slope,
// and edges get a soft bevel; flat faces are unchanged.
fn smooth_normal(v: vec3<i32>, m: u32, face: vec3<f32>) -> vec3<f32> {
    let k = frame.lighting.z;
    if k <= 0.0 {
        return face;
    }
    let n = vec3<i32>(round(face));
    var u = vec3<i32>(0, 1, 0);
    var w = vec3<i32>(0, 0, 1);
    if n.y != 0 {
        u = vec3<i32>(1, 0, 0);
    } else if n.z != 0 {
        u = vec3<i32>(1, 0, 0);
        w = vec3<i32>(0, 1, 0);
    }
    // Slope along u and w: height differences weighted by 1/distance.
    var su = 0.0;
    var sw = 0.0;
    for (var d = 1; d <= 3; d++) {
        let inv = 1.0 / f32(d);
        su += (relative_height(v + u * d, n, m) - relative_height(v - u * d, n, m)) * inv;
        sw += (relative_height(v + w * d, n, m) - relative_height(v - w * d, n, m)) * inv;
    }
    // Normalised so a 45° staircase gives about a 45° normal.
    let scale = k / 3.67;
    return normalize(face - (vec3<f32>(u) * su + vec3<f32>(w) * sw) * scale);
}

fn temperature_at(v: vec3<i32>) -> f32 {
    let dims = vec3<i32>(frame.volume_dims.xyz);
    let c = v / CHUNK;
    let slot = heat_index[c.x + c.y * dims.x + c.z * dims.x * dims.y];
    if slot == EMPTY {
        return frame.fog.z;
    }
    let l = (v - c * CHUNK) / 4;
    return heat_pool[slot * 4096u + u32(l.x + l.y * 16 + l.z * 256)];
}

// Debug colour ramp: blue (cold) - grey (ambient) - yellow (100 °C) - orange - white.
fn heat_color(t: f32, ambient: f32) -> vec3<f32> {
    if t < ambient {
        let k = clamp((ambient - t) / (ambient + 20.0), 0.0, 1.0);
        return mix(vec3<f32>(0.25), vec3<f32>(0.05, 0.25, 1.0), k);
    }
    if t < 100.0 {
        return mix(vec3<f32>(0.25), vec3<f32>(1.0, 0.85, 0.1), (t - ambient) / (100.0 - ambient));
    }
    if t < 600.0 {
        return mix(vec3<f32>(1.0, 0.85, 0.1), vec3<f32>(1.0, 0.3, 0.02), (t - 100.0) / 500.0);
    }
    return mix(vec3<f32>(1.0, 0.3, 0.02), vec3<f32>(1.0), clamp((t - 600.0) / 900.0, 0.0, 1.0));
}

const SUN_COLOR: vec3<f32> = vec3<f32>(1.0, 0.92, 0.78);

fn sky(rd: vec3<f32>) -> vec3<f32> {
    let horizon = vec3<f32>(0.62, 0.74, 0.88);
    let zenith = vec3<f32>(0.18, 0.36, 0.72);
    let ground = vec3<f32>(0.22, 0.22, 0.24);
    var c: vec3<f32>;
    if rd.y < 0.0 {
        c = mix(horizon, ground, min(-rd.y * 4.0, 1.0));
    } else {
        c = mix(horizon, zenith, pow(rd.y, 0.6));
    }
    // Warm glow around the sun.
    let s = max(dot(rd, frame.sun_dir.xyz), 0.0);
    return c + SUN_COLOR * (0.25 * pow(s, 8.0));
}

fn sky_with_sun(rd: vec3<f32>) -> vec3<f32> {
    let s = dot(rd, frame.sun_dir.xyz);
    return sky(rd) + SUN_COLOR * 20.0 * smoothstep(0.9994, 0.9998, s);
}

// Fraction of sunlight reaching `p`, a point on a face with normal `n`. The shadow
// ray starts just off the face; on a face turned away from the sun (lit only through
// its smoothed normal) it starts at the neighbouring air voxel along `ns` instead.
fn sun_visibility(p: vec3<f32>, n: vec3<f32>, ns: vec3<f32>) -> f32 {
    let dist = frame.lighting.x;
    if dist <= 0.0 {
        return 1.0;
    }
    var o = p + n * 0.02;
    if dot(n, frame.sun_dir.xyz) <= 0.0 {
        o = p + n * 0.5 + ns * 0.5;
    }
    let sd = frame.sun_dir.xyz;
    var hit = trace(o, sd, 0u, dist);
    var light = 1.0;
    // Translucent material lets some light through; look past it once.
    if hit.found && !is_opaque(hit.material) {
        light = 1.0 - materials[hit.material].color.a * 0.6;
        hit = trace(o, sd, hit.material, dist);
    }
    if hit.found {
        return 0.0;
    }
    if trace_bodies(o, sd, dist).found {
        return 0.0;
    }
    return light;
}

// Smooth voxel-scale ambient occlusion of the face of voxel `v` with normal `n` at
// point `p`: occupancy of the 8 voxels around the face, interpolated per corner.
fn corner_ao(v: vec3<i32>, n: vec3<f32>, p: vec3<f32>) -> f32 {
    let ni = vec3<i32>(round(n));
    var u = vec3<i32>(0, 1, 0);
    var w = vec3<i32>(0, 0, 1);
    if ni.y != 0 {
        u = vec3<i32>(1, 0, 0);
    } else if ni.z != 0 {
        u = vec3<i32>(1, 0, 0);
        w = vec3<i32>(0, 1, 0);
    }
    let base = v + ni;
    var occ: array<f32, 9>;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            occ[(j + 1) * 3 + (i + 1)] = select(0.0, 1.0, is_opaque(voxel_at(base + u * i + w * j)));
        }
    }
    // Position inside the face, 0..1 along u and w.
    let f = p - vec3<f32>(v);
    let fu = clamp(dot(f, vec3<f32>(u)), 0.0, 1.0);
    let fw = clamp(dot(f, vec3<f32>(w)), 0.0, 1.0);
    // Corner value: 0 occluders -> 1, both sides -> 0.25.
    var c: array<f32, 4>;
    for (var k = 0; k < 4; k++) {
        let su = select(-1, 1, (k & 1) != 0);
        let sw = select(-1, 1, (k & 2) != 0);
        let side1 = occ[4 + su];
        let side2 = occ[4 + sw * 3];
        let corner = occ[4 + su + sw * 3];
        let blocked = select(side1 + side2 + corner, 3.0, side1 + side2 >= 2.0);
        c[k] = 1.0 - blocked * 0.25;
    }
    return mix(mix(c[0], c[1], fu), mix(c[2], c[3], fu), fw);
}

struct Ambient {
    occlusion: f32,       // 1 = open sky, 0 = enclosed
    glow: vec3<f32>,      // light gathered from emissive voxels nearby
};

// Larger-scale occlusion with short rays sampled at a few distances. Samples that
// land in emissive voxels add their light (lava lights up its surroundings).
fn ray_ambient(p: vec3<f32>, n: vec3<f32>) -> Ambient {
    var a = Ambient(1.0, vec3<f32>(0.0));
    let strength = frame.lighting.y;
    if strength <= 0.0 {
        return a;
    }
    // Tangent frame around the normal.
    let helper = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(n.x) > 0.5);
    let t1 = normalize(helper - n * dot(n, helper));
    let t2 = cross(n, t1);
    let dirs = array<vec3<f32>, 5>(
        n,
        normalize(n + t1 * 0.9),
        normalize(n - t1 * 0.9),
        normalize(n + t2 * 0.9),
        normalize(n - t2 * 0.9),
    );
    let dists = array<f32, 4>(2.5, 5.0, 9.0, 15.0);
    var occluded = 0.0;
    for (var r = 0; r < 5; r++) {
        for (var s = 0; s < 4; s++) {
            let q = p + n * 0.5 + dirs[r] * dists[s];
            let m = voxel_at(vec3<i32>(floor(q)));
            if m != 0u {
                let mat = materials[m];
                let fall = 1.0 - f32(s) * 0.22;
                if mat.params.x > 0.0 {
                    a.glow += mat.color.rgb * mat.params.x * fall * 0.25;
                }
                if mat.color.a >= OPAQUE_ALPHA {
                    occluded += fall;
                    break;
                }
            }
        }
    }
    a.occlusion = 1.0 - strength * occluded / 5.0;
    return a;
}

// Lit colour of a surface hit at distance `t` along `rd` from `ro`.
fn shade(s: Surface, ro: vec3<f32>, rd: vec3<f32>) -> vec3<f32> {
    let mat = materials[s.material];
    let p = ro + rd * s.t;
    let n = s.normal;
    // Brightness texture: broad patches (smooth noise over ~6 voxels) plus a little
    // per-voxel noise (`frame.lighting.w`). Strong per-voxel noise made every voxel
    // stand out as a separate cube.
    let fine = (hash3(s.seed) - 0.5) * 0.18 * frame.lighting.w;
    let broad = (value_noise(vec3<f32>(s.seed) / 6.0) - 0.5) * 0.14;
    let base = mat.color.rgb * (0.93 + broad + fine);
    let emissive = mat.params.x;
    if emissive > 0.0 {
        // Self-lit: no shadows, glows into the HDR buffer.
        return base * (0.5 + emissive * 0.35);
    }
    // Lighting uses the smoothed normal, ray offsets the face normal.
    var ns = n;
    if !s.is_body {
        ns = smooth_normal(s.voxel, s.material, n);
    }
    var ao = 1.0;
    var glow = vec3<f32>(0.0);
    if frame.lighting.y > 0.0 {
        if !s.is_body {
            ao = corner_ao(s.voxel, n, p);
        }
        let amb = ray_ambient(p, n);
        ao *= amb.occlusion;
        glow = amb.glow;
    }
    let ndl = dot(ns, frame.sun_dir.xyz);
    var sun = 0.0;
    if ndl > 0.0 {
        sun = ndl * sun_visibility(p, n, ns);
    }
    // Sky light from above, a little bounce from below.
    let sky_light = mix(vec3<f32>(0.16, 0.16, 0.18), vec3<f32>(0.34, 0.42, 0.56), ns.y * 0.5 + 0.5);
    return base * (sky_light * ao + SUN_COLOR * 0.85 * sun + glow);
}

// Distance the ray travels through contiguous voxels of material `m`, starting at
// `p` (just inside the first one), up to `max_steps` voxels.
fn material_run(p: vec3<f32>, rd_in: vec3<f32>, m: u32, max_steps: i32) -> f32 {
    let rd = select(rd_in, vec3<f32>(1e-8), abs(rd_in) < vec3<f32>(1e-8));
    let inv = 1.0 / rd;
    let step = vec3<i32>(sign(rd));
    var v = vec3<i32>(floor(p));
    let delta = abs(inv);
    var t_max = (vec3<f32>(v) + select(vec3<f32>(0.0), vec3<f32>(1.0), rd > vec3<f32>(0.0)) - p) * inv;
    var t = 0.0;
    for (var i = 0; i < max_steps; i++) {
        let axis = min_axis(t_max);
        t = t_max[axis];
        t_max[axis] += delta[axis];
        v[axis] += step[axis];
        if voxel_at(v) != m {
            return t;
        }
    }
    return t;
}

fn apply_fog(color: vec3<f32>, rd: vec3<f32>, t: f32) -> vec3<f32> {
    let ft = max(t - frame.fog.y, 0.0) * frame.fog.x;
    return mix(color, sky(rd), 1.0 - exp(-ft * ft));
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(frame.projection.zw);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let tan_half = frame.projection.x;
    let aspect = frame.projection.y;
    let rd = normalize(frame.forward.xyz
        + ndc.x * tan_half * aspect * frame.right.xyz
        + ndc.y * tan_half * frame.up.xyz);
    let ro = frame.camera_pos.xyz;

    var color = sky_with_sun(rd);
    var s = find_surface(ro, rd, 0u, FAR);
    if s.found {
        let mat = materials[s.material];
        // Liquid surfaces: smooth normal from the neighbouring fill levels.
        if mat.params.y > 0.0 && !s.is_body && s.normal.y > 0.5 {
            s.normal = liquid_normal(s.voxel, s.material);
        }
        color = shade(s, ro, rd);

        // One layer of transparency: look past the translucent material and blend by
        // the thickness crossed (Beer-Lambert), plus a sky reflection on liquids.
        if mat.color.a < OPAQUE_ALPHA && !s.is_body {
            let p = ro + rd * (s.t + 1e-3);
            // Absorption only over the material itself (a droplet is thin even if the
            // ground behind it is far), the colour from whatever is behind.
            let thickness = material_run(p, rd, s.material, 64);
            let behind = find_surface(p, rd, s.material, FAR);
            var back = sky(rd);
            if behind.found {
                back = shade(behind, p, rd);
            }
            // Alpha is the opacity of ~8 voxels of the material.
            let k = -log(max(1.0 - mat.color.a, 0.02)) / 8.0;
            let transmit = exp(-k * thickness);
            color = mix(color, back, transmit);
            if mat.params.y > 0.0 {
                let cos_i = clamp(-dot(rd, s.normal), 0.0, 1.0);
                let fresnel = 0.02 + 0.98 * pow(1.0 - cos_i, 5.0);
                color = mix(color, sky(reflect(rd, s.normal)), fresnel * 0.8);
            }
        }

        if !s.is_body && (frame.volume_dims.w & 1u) != 0u {
            let face = 0.88 + 0.12 * s.normal.y;
            let heat = heat_color(temperature_at(s.voxel), frame.fog.z);
            color = heat * (0.55 + 0.45 * face) * 2.0;
        }

        // Brush preview: tint voxels inside the brush sphere.
        if !s.is_body && frame.brush.w > 0.0 {
            let d = vec3<f32>(s.voxel) + 0.5 - frame.brush.xyz;
            if dot(d, d) <= frame.brush.w * frame.brush.w {
                color = mix(color, vec3<f32>(1.0, 0.85, 0.3), 0.35);
            }
        }

        color = apply_fog(color, rd, s.t);
    }
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(color, 1.0));
}
