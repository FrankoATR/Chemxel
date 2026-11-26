// Copies the HDR raymarch output to the swapchain with a filmic tonemap, over
// which it composites the particles (premultiplied alpha). The swapchain is sRGB,
// so the hardware applies the sRGB encoding on write.

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var particles: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // Fullscreen triangle: (-1,-1), (3,-1), (-1,3).
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// ACES filmic approximation (Krzysztof Narkowicz).
fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Part of a pixel's HDR colour that "glows" (above 1).
fn bright(p: vec2<i32>, size: vec2<i32>) -> vec3<f32> {
    let c = textureLoad(source, clamp(p, vec2<i32>(0), size - 1), 0).rgb;
    return max(c - vec3<f32>(1.0), vec3<f32>(0.0));
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(position.xy);
    let size = vec2<i32>(textureDimensions(source));
    let hdr = textureLoad(source, p, 0).rgb;
    // Cheap bloom: the bright excess of two rings of neighbours makes lava and fire
    // glow (milestone 7).
    var glow = vec3<f32>(0.0);
    for (var i = 0; i < 8; i++) {
        let a = f32(i) * 0.785398;
        let d = vec2<f32>(cos(a), sin(a));
        glow += bright(p + vec2<i32>(d * 3.0), size) * 0.6;
        glow += bright(p + vec2<i32>(d * 9.0), size) * 0.4;
    }
    let part = textureLoad(particles, p, 0);
    let color = (hdr + glow * (0.18 / 8.0)) * (1.0 - part.a) + part.rgb;
    return vec4<f32>(aces(color * 1.1), 1.0);
}
