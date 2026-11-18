// Voxel raymarcher: one thread per pixel, three-level DDA (Amanatides & Woo):
// chunks (64³) → bricks (8³) → voxels.
//
// Data layout (mirrors mv_render::gpu_volume):
//   chunk_grid[c]                     entry for chunk cell c of the window
//   chunk_bricks[slot * 512 + b]      entry for brick b of the chunk at `slot`
//   brick_pool[brick * 256 + i / 2]   two u16 material ids per u32 (low half = even i)
//   materials[id]                     linear colour + emissive
// Grid and table entries: EMPTY (air), UNIFORM_BIT | material (whole chunk/brick is
// one material), or a slot index into the next level.
// Index orders are always x-fastest: x + y*N + z*N*N.
// Coordinates are in voxels, relative to the volume's minimum corner.
//
// Rigid bodies (mirrors mv_render::bodies): body_headers[i] has the inverse rotation
// (rows), the position and the size + voxel offset of a dense grid stored in
// body_voxels (two u16 per u32). They are traced after the world; the nearest hit
// wins.

const EMPTY: u32 = 0xffffffffu;
const UNIFORM_BIT: u32 = 0x80000000u;
const BRICK: i32 = 8;
const BRICKS_PER_CHUNK_AXIS: i32 = 8;
const CHUNK: i32 = 64;
const MAX_CHUNK_STEPS: u32 = 512u;

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

struct Material {
    color: vec4<f32>,  // linear rgb + alpha
    params: vec4<f32>, // x: emissive
};

struct Hit {
    found: bool,
    t: f32,
    material: u32,
    axis: i32,        // axis of the face that was hit (0 = x, 1 = y, 2 = z)
    voxel: vec3<i32>,
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

fn voxel_material(brick: u32, v: vec3<i32>) -> u32 {
    let i = u32(v.x + v.y * 8 + v.z * 64);
    let word = brick_pool[brick * 256u + (i >> 1u)];
    return (word >> ((i & 1u) * 16u)) & 0xffffu;
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

// Voxel-level DDA inside one 8³ brick, starting at ray parameter t_start.
fn trace_brick(ro: vec3<f32>, rd: vec3<f32>, inv: vec3<f32>, step: vec3<i32>,
               t_start: f32, bmin: vec3<i32>, brick: u32, entry_axis: i32) -> Hit {
    let bmax = bmin + vec3<i32>(BRICK - 1);
    let p = ro + rd * t_start;
    var v = clamp(vec3<i32>(floor(p)), bmin, bmax);
    let delta = abs(inv);
    var t_max = (vec3<f32>(v) + select(vec3<f32>(0.0), vec3<f32>(1.0), rd > vec3<f32>(0.0)) - ro) * inv;
    var axis = entry_axis;
    var t = t_start;
    for (var i = 0; i < 3 * BRICK; i++) {
        let m = voxel_material(brick, v - bmin);
        if m != 0u {
            return Hit(true, t, m, axis, v);
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
               t_start: f32, t_exit: f32, cmin: vec3<i32>, slot: u32, entry_axis: i32) -> Hit {
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
                // Solid uniform brick: the entry point is the hit.
                let v = clamp(vec3<i32>(floor(ro + rd * t)), bmin, bmin + vec3<i32>(BRICK - 1));
                return Hit(true, t, entry & 0xffffu, axis, v);
            }
            let hit = trace_brick(ro, rd, inv, step, t, bmin, entry, axis);
            if hit.found {
                return hit;
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
// chunks hit immediately, the rest descend to bricks and then voxels.
fn trace(ro: vec3<f32>, rd_in: vec3<f32>) -> Hit {
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
    let t_exit = min(min(t_far.x, t_far.y), t_far.z);
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
                let v = clamp(vec3<i32>(floor(ro + rd * t)), cmin, cmin + vec3<i32>(CHUNK - 1));
                return Hit(true, t, entry & 0xffffu, axis, v);
            }
            let hit = trace_chunk(ro, rd, inv, step, t, t_exit, cmin, entry, axis);
            if hit.found {
                return hit;
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

fn hash3(p: vec3<i32>) -> f32 {
    // WGSL forbids mixing bitwise and arithmetic operators without parentheses.
    var h = (u32(p.x) * 73856093u) ^ (u32(p.y) * 19349663u) ^ (u32(p.z) * 83492791u);
    h = (h ^ (h >> 13u)) * 1274126177u;
    return f32(h & 0xffffu) / 65535.0;
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

fn sky(rd: vec3<f32>) -> vec3<f32> {
    let horizon = vec3<f32>(0.62, 0.74, 0.88);
    let zenith = vec3<f32>(0.18, 0.36, 0.72);
    let ground = vec3<f32>(0.22, 0.22, 0.24);
    if rd.y < 0.0 {
        return mix(horizon, ground, min(-rd.y * 4.0, 1.0));
    }
    return mix(horizon, zenith, pow(rd.y, 0.6));
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

    var color = sky(rd);
    let hit = trace(ro, rd);
    let body = trace_bodies(ro, rd, select(1e30, hit.t, hit.found));
    if hit.found || body.found {
        var normal = vec3<f32>(0.0);
        var material = hit.material;
        var seed = hit.voxel;
        var t = hit.t;
        if body.found {
            normal = body.normal;
            material = body.material;
            seed = body.seed;
            t = body.t;
        } else {
            normal[hit.axis] = -sign(rd[hit.axis]);
        }
        let mat = materials[material];

        // Fixed per-face ambient makes voxel edges readable before real lighting:
        // 1.0 on top faces, 0.88 on sides, 0.72 below (smooth for rotated bodies).
        let face_ambient = select(0.88 + 0.16 * normal.y, 0.88 + 0.12 * normal.y, normal.y > 0.0);
        let diffuse = max(dot(normal, frame.sun_dir.xyz), 0.0);
        let variation = 0.9 + 0.1 * hash3(seed);
        let base = mat.color.rgb * variation;
        color = base * (0.35 * face_ambient + 0.75 * diffuse) + base * mat.params.x * 0.6;

        if !body.found && (frame.volume_dims.w & 1u) != 0u {
            let heat = heat_color(temperature_at(hit.voxel), frame.fog.z);
            color = heat * (0.55 + 0.45 * face_ambient) * 2.0;
        }

        // Brush preview: tint voxels inside the brush sphere.
        if !body.found && frame.brush.w > 0.0 {
            let d = vec3<f32>(hit.voxel) + 0.5 - frame.brush.xyz;
            if dot(d, d) <= frame.brush.w * frame.brush.w {
                color = mix(color, vec3<f32>(1.0, 0.85, 0.3), 0.35);
            }
        }

        // Distance fog, fully opaque near the edge of the render distance.
        let ft = max(t - frame.fog.y, 0.0) * frame.fog.x;
        color = mix(color, sky(rd), 1.0 - exp(-ft * ft));
    }
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(color, 1.0));
}
