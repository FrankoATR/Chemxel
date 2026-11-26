// Particles (spray, foam, sparks) as camera-facing round billboards, drawn into
// their own premultiplied-alpha target and composited by blit.wgsl. The raymarch
// output keeps the distance to the first surface in its alpha channel, so particles
// behind voxels are discarded per pixel (ADR 015).

struct Frame {
    camera_pos: vec4<f32>,
    forward: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    projection: vec4<f32>,  // x: tan(fov_y / 2), y: aspect ratio, z: width, w: height
    sun_dir: vec4<f32>,
    brush: vec4<f32>,
    fog: vec4<f32>,         // x: k, y: start s in 1 - exp(-((t - s) * k)^2)
    volume_dims: vec4<u32>,
    bodies: vec4<u32>,
    lighting: vec4<f32>,
};

struct Particle {
    pos_size: vec4<f32>,  // xyz: centre (volume-relative voxels), w: radius
    color: vec4<f32>,     // linear rgb (may exceed 1: glowing sparks), a: opacity
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> particles: array<Particle>;
@group(0) @binding(2) var scene: texture_2d<f32>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) dist: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let p = particles[ii];
    // Triangle strip: (-1,-1), (1,-1), (-1,1), (1,1).
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u)) * 2.0 - 1.0;
    let centre = p.pos_size.xyz - frame.camera_pos.xyz;
    let d = centre + (frame.right.xyz * corner.x + frame.up.xyz * corner.y) * p.pos_size.w;
    let z = dot(d, frame.forward.xyz);
    var out: VsOut;
    out.uv = corner;
    out.color = p.color;
    out.dist = length(centre);
    if z < 0.05 {
        // Behind the camera: outside the clip volume.
        out.clip = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        return out;
    }
    let tan_half = frame.projection.x;
    let aspect = frame.projection.y;
    out.clip = vec4<f32>(dot(d, frame.right.xyz) / (tan_half * aspect), dot(d, frame.up.xyz) / tan_half, 0.5 * z, z);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let r2 = dot(in.uv, in.uv);
    if r2 > 1.0 {
        discard;
    }
    // Hidden behind the first surface the ray hit.
    let depth = textureLoad(scene, vec2<i32>(in.clip.xy), 0).a;
    if in.dist > depth + 0.5 {
        discard;
    }
    let ft = max(in.dist - frame.fog.y, 0.0) * frame.fog.x;
    let a = in.color.a * (1.0 - r2) * exp(-ft * ft);
    return vec4<f32>(in.color.rgb * a, a);
}
