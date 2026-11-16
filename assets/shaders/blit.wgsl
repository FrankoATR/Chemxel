// Copies the HDR raymarch output to the swapchain with a filmic tonemap.
// The swapchain is sRGB, so the hardware applies the sRGB encoding on write.

@group(0) @binding(0) var source: texture_2d<f32>;

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

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let hdr = textureLoad(source, vec2<i32>(position.xy), 0).rgb;
    return vec4<f32>(aces(hdr * 1.1), 1.0);
}
