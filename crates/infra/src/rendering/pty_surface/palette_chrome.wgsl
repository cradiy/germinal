struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) shape: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32, @location(0) rect: vec4<f32>, @location(1) color: vec4<f32>, @location(2) params: vec4<f32>) -> Vertex {
    let corners = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    let local = corners[index] * rect.zw;
    let point = (rect.xy + local) / params.zw;
    return Vertex(vec4(point.x * 2.0 - 1.0, 1.0 - point.y * 2.0, 0.0, 1.0), local, rect.zw, color, params.xy);
}

@fragment
fn fs_main(vertex: Vertex) -> @location(0) vec4<f32> {
    let radius = min(vertex.shape.x, min(vertex.size.x, vertex.size.y) * 0.5);
    let q = abs(vertex.local - vertex.size * 0.5) - vertex.size * 0.5 + vec2(radius);
    let distance = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    let coverage = 1.0 - smoothstep(-vertex.shape.y, vertex.shape.y, distance);
    return vec4(vertex.color.rgb, vertex.color.a * coverage);
}
