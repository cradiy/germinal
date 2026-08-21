fn star_hash(point: vec2<f32>) -> f32 {
    let value = dot(point, vec2<f32>(127.1, 311.7));
    return fract(sin(value) * 43758.5453);
}

fn star_dust(pixel: vec2<f32>) -> vec3<f32> {
    let cell_size = 24.0;
    let cell = floor(pixel / cell_size);
    let local = fract(pixel / cell_size);
    let position = vec2<f32>(
        0.12 + 0.76 * star_hash(cell + vec2<f32>(19.3, 7.1)),
        0.12 + 0.76 * star_hash(cell + vec2<f32>(3.7, 29.9)),
    );
    let distance_px = length((local - position) * cell_size);
    let seed = star_hash(cell);
    let visible = step(0.78, seed);
    let radius = 0.32 + 0.48 * star_hash(cell + vec2<f32>(41.0, 13.0));
    let point = 1.0 - smoothstep(radius * 0.25, radius, distance_px);
    let brightness = 0.2 + 0.38 * star_hash(cell + vec2<f32>(11.0, 83.0));
    let color = mix(
        vec3<f32>(0.38, 0.5, 0.78),
        vec3<f32>(0.82, 0.88, 1.0),
        star_hash(cell + vec2<f32>(73.0, 5.0)),
    );
    return color * point * visible * brightness;
}

fn meteor(
    pixel: vec2<f32>,
    resolution: vec2<f32>,
    time: f32,
    generation: f32,
    lane: f32,
) -> vec3<f32> {
    let id = vec2<f32>(generation, lane * 17.0 + 3.0);
    let enabled = step(0.43, star_hash(id + vec2<f32>(5.0, 31.0)));
    let interval = 0.58;
    let born = generation * interval +
        star_hash(id + vec2<f32>(17.0, 43.0)) * interval;
    let age = time - born;
    let lifetime = 1.15 + 0.85 * star_hash(id + vec2<f32>(61.0, 7.0));
    let alive = step(0.0, age) * step(age, lifetime) * enabled;

    let direction = normalize(vec2<f32>(
        -1.0,
        0.54 + 0.22 * star_hash(id + vec2<f32>(23.0, 71.0)),
    ));
    let speed = 250.0 + 190.0 * star_hash(id + vec2<f32>(37.0, 19.0));
    let tail_length = 105.0 + 150.0 * star_hash(id + vec2<f32>(89.0, 13.0));
    let margin = tail_length + 90.0;
    let start = vec2<f32>(
        -margin * 0.2 +
            star_hash(id + vec2<f32>(47.0, 97.0)) * (resolution.x + margin * 1.4),
        -margin * 0.85 +
            star_hash(id + vec2<f32>(79.0, 53.0)) * (resolution.y + margin * 1.15),
    );
    let head = start + direction * speed * age;
    let relative = pixel - head;
    let behind = dot(relative, -direction);
    let normal = vec2<f32>(-direction.y, direction.x);
    let across = abs(dot(relative, normal));
    let width = 0.8 + 1.2 * star_hash(id + vec2<f32>(101.0, 29.0));
    let grown_tail = max(1.0, min(tail_length, speed * max(age, 0.0)));
    let tail_bounds = step(0.0, behind) * step(behind, grown_tail);
    let tail_falloff = pow(clamp(1.0 - behind / grown_tail, 0.0, 1.0), 1.7);
    let tail_core = exp(-pow(across / width, 2.0)) * tail_falloff * tail_bounds;
    let tail_glow = exp(-pow(across / (width * 4.5), 2.0)) *
        pow(tail_falloff, 1.4) * tail_bounds;

    let head_radius = width * 2.35;
    let head_core = exp(-dot(relative, relative) / (head_radius * head_radius));
    let head_glow = exp(-dot(relative, relative) /
        (head_radius * head_radius * 8.0));
    let fade_in = smoothstep(0.0, 0.12, age);
    let fade_out = 1.0 - smoothstep(lifetime * 0.68, lifetime, age);
    let fade = alive * fade_in * fade_out;
    let intensity = 0.68 + 0.42 * star_hash(id + vec2<f32>(109.0, 59.0));

    let blue_glow = vec3<f32>(0.06, 0.18, 0.58) * tail_glow * 0.72;
    let blue_core = vec3<f32>(0.38, 0.62, 1.0) * tail_core;
    let white_head = vec3<f32>(0.9, 0.96, 1.0) * head_core +
        vec3<f32>(0.18, 0.42, 1.0) * head_glow * 0.52;
    return (blue_glow + blue_core + white_head) * fade * intensity;
}

fn background(uv: vec2<f32>, time: f32, resolution: vec2<f32>) -> vec4<f32> {
    let pixel = uv * resolution;
    var color = vec3<f32>(0.0015, 0.0025, 0.006);
    color += star_dust(pixel);

    let current_generation = floor(time / 0.58);
    for (var generation_offset = 0; generation_offset < 5; generation_offset += 1) {
        let generation = current_generation - f32(generation_offset);
        for (var lane = 0; lane < 5; lane += 1) {
            color += meteor(pixel, resolution, time, generation, f32(lane));
        }
    }

    return vec4<f32>(color, 1.0);
}
