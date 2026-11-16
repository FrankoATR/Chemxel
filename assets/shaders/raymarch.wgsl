// Voxel raymarcher: one thread per pixel, hierarchical DDA (Amanatides & Woo).
//
// Data layout (mirrors mv_render::gpu_volume):
//   chunk_grid[c]                     chunk slot for grid cell c, or EMPTY
//   chunk_bricks[slot * 512 + b]      brick slot for brick b of a chunk, or EMPTY
//   brick_pool[brick * 256 + i / 2]   two u16 material ids per u32 (low half = even i)
//   materials[id]                     linear colour + emissive
// Index orders are always x-fastest: x + y*N + z*N*N.
// Coordinates are in voxels, relative to the volume's minimum corner.

const EMPTY: u32 = 0xffffffffu;
const BRICK: i32 = 8;
const BRICKS_PER_CHUNK_AXIS: i32 = 8;
const MAX_BRICK_STEPS: u32 = 1024u;

struct Frame {
    camera_pos: vec4<f32>,  // xyz: eye position relative to the volume origin
    forward: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    // x: tan(fov_y / 2), y: aspect ratio, z: width, w: height
    projection: vec4<f32>,
    sun_dir: vec4<f32>,     // xyz: direction towards the sun (normalised)
    volume_dims: vec4<u32>, // xyz: volume size in chunks
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

fn no_hit() -> Hit {
    return Hit(false, 0.0, 0u, 0, vec3<i32>(0));
}

fn brick_slot_at(g: vec3<i32>) -> u32 {
    let dims = vec3<i32>(frame.volume_dims.xyz);
    let c = g / BRICKS_PER_CHUNK_AXIS;
    let chunk = chunk_grid[c.x + c.y * dims.x + c.z * dims.x * dims.y];
    if chunk == EMPTY {
        return EMPTY;
    }
    let b = g - c * BRICKS_PER_CHUNK_AXIS;
    return chunk_bricks[chunk * 512u + u32(b.x + b.y * 8 + b.z * 64)];
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

fn trace(ro: vec3<f32>, rd_in: vec3<f32>) -> Hit {
    // Avoid divisions by zero for axis-aligned rays.
    let eps = 1e-8;
    let rd = select(rd_in, vec3<f32>(eps), abs(rd_in) < vec3<f32>(eps));
    let inv = 1.0 / rd;

    let grid = vec3<i32>(frame.volume_dims.xyz) * BRICKS_PER_CHUNK_AXIS;
    let bounds = vec3<f32>(grid * BRICK);
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
    var g = clamp(vec3<i32>(floor(p / f32(BRICK))), vec3<i32>(0), grid - 1);
    let delta = abs(f32(BRICK) * inv);
    var t_max = ((vec3<f32>(g) + select(vec3<f32>(0.0), vec3<f32>(1.0), rd > vec3<f32>(0.0))) * f32(BRICK) - ro) * inv;
    var t = t_enter;

    for (var i = 0u; i < MAX_BRICK_STEPS; i++) {
        let brick = brick_slot_at(g);
        if brick != EMPTY {
            let hit = trace_brick(ro, rd, inv, step, t, g * BRICK, brick, axis);
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
        g[axis] += step[axis];
        if g[axis] < 0 || g[axis] >= grid[axis] {
            break;
        }
    }
    return no_hit();
}

fn hash3(p: vec3<i32>) -> f32 {
    // WGSL forbids mixing bitwise and arithmetic operators without parentheses.
    var h = (u32(p.x) * 73856093u) ^ (u32(p.y) * 19349663u) ^ (u32(p.z) * 83492791u);
    h = (h ^ (h >> 13u)) * 1274126177u;
    return f32(h & 0xffffu) / 65535.0;
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
    if hit.found {
        let mat = materials[hit.material];
        var normal = vec3<f32>(0.0);
        normal[hit.axis] = -sign(rd[hit.axis]);

        // Fixed per-face ambient makes voxel edges readable before real lighting.
        let face_ambient = select(select(0.72, 0.88, hit.axis != 1), 1.0, hit.axis == 1 && normal.y > 0.0);
        let diffuse = max(dot(normal, frame.sun_dir.xyz), 0.0);
        let variation = 0.9 + 0.1 * hash3(hit.voxel);
        let base = mat.color.rgb * variation;
        color = base * (0.35 * face_ambient + 0.75 * diffuse) + base * mat.params.x * 0.6;

        // Distance fog.
        let fog = 1.0 - exp(-hit.t * 0.0015);
        color = mix(color, sky(rd), fog);
    }
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(color, 1.0));
}
