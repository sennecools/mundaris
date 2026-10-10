// Auto exposure: 256-bin log-luminance histogram and EV100 eye adaptation.
// Exposure maps the metered luminance to middle grey: e = 0.18 / L = 1.44 / 2^EV100.

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var hdr_tex: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> histogram: array<atomic<u32>, 256>;
@group(0) @binding(3) var<storage, read_write> exposure: Exposure;

const MIN_LOG2: f32 = -10.0;
const LOG2_RANGE: f32 = 34.0;

var<workgroup> local_bins: array<atomic<u32>, 256>;
var<workgroup> counts: array<u32, 256>;

@compute @workgroup_size(16, 16)
fn histogram_main(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    atomicStore(&local_bins[li], 0u);
    workgroupBarrier();
    let dims = textureDimensions(hdr_tex);
    if id.x < dims.x && id.y < dims.y && post.tonemap.w < 0.5 {
        let c = textureLoad(hdr_tex, vec2<i32>(id.xy), 0).rgb;
        // Pre-exposed radiance back to luminance in cd/m².
        let lum = luminance(c) / max(exposure.value, 1.0e-20);
        if lum > 1.0e-3 {
            let t = clamp((log2(lum) - MIN_LOG2) / LOG2_RANGE, 0.0, 1.0);
            atomicAdd(&local_bins[u32(t * 254.0) + 1u], 1u);
        }
    }
    workgroupBarrier();
    let count = atomicLoad(&local_bins[li]);
    if count > 0u {
        atomicAdd(&histogram[li], count);
    }
}

fn bin_log2(bin: u32) -> f32 {
    return (f32(bin - 1u) + 0.5) / 254.0 * LOG2_RANGE + MIN_LOG2;
}

@compute @workgroup_size(256)
fn adapt_main(@builtin(local_invocation_index) li: u32) {
    counts[li] = atomicLoad(&histogram[li]);
    atomicStore(&histogram[li], 0u);
    workgroupBarrier();
    if li != 0u {
        return;
    }
    var total = 0u;
    for (var b = 1u; b < 256u; b++) {
        total += counts[b];
    }
    let previous_ev = exposure.ev100;
    var ev = previous_ev;
    var metered = exposure.luminance;
    if post.exposure.x > 0.5 {
        ev = post.exposure.y;
    } else if total > 0u {
        // Mean log luminance of the 55th..98th percentile band: weighted
        // towards lit surfaces so sunlit terrain does not clip.
        let low = f32(total) * 0.55;
        let high = f32(total) * 0.98;
        var seen = 0.0;
        var sum = 0.0;
        var weight = 0.0;
        for (var b = 1u; b < 256u; b++) {
            let c = f32(counts[b]);
            let band_start = max(seen, low);
            let band_end = min(seen + c, high);
            if band_end > band_start {
                sum += (band_end - band_start) * bin_log2(b);
                weight += band_end - band_start;
            }
            seen += c;
        }
        let average = select(previous_ev, sum / max(weight, 1.0), weight > 0.0);
        metered = exp2(average);
        let goal = clamp(log2(metered * 8.0), post.exposure_range.x, post.exposure_range.y);
        if exposure.luminance < 0.0 {
            ev = goal;
        } else {
            // Exponential approach: each frame closes 1 - e^(-rate·dt) of the
            // gap (rate per second, separate towards bright and dark), so a
            // night-to-day jump settles in about a second and small changes
            // stay smooth.
            let dt = post.exposure.w;
            let rate = select(post.exposure_range.w, post.exposure_range.z, goal > previous_ev);
            ev = previous_ev + (goal - previous_ev) * (1.0 - exp(-rate * dt));
        }
    }
    exposure.previous = exposure.value;
    exposure.ev100 = ev;
    exposure.value = 1.44 / exp2(ev) * exp2(post.exposure.z);
    exposure.luminance = max(metered, 0.0);
}
