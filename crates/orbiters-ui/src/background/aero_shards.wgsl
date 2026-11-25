// React Bits, Copyright (c) 2026 David Haz. MIT + Commons Clause (assets/licenses/react-bits.txt).
// Aero Shards as orbiters.cc draws it: the shaders of `vendor/AeroShards.jsx`, word for word (its two constants written in).

struct ViewParams {
  viewport: vec4f,
  shape: vec4f,
  effects: vec4f,
  composition: vec4f,
  transport: vec4f,
  formation: vec4f,
  gather: vec4f,
  pointer: vec4f,
  shock: vec4f,
  shockB: vec4f,
  shockC: vec4f,
  shockD: vec4f,
  material: vec4f,
  light: vec4f,
  environment: vec4f,
  baseColor: vec4f,
  highlightColor: vec4f,
  accentColor: vec4f,
}

struct PathSample {
  position: vec3f,
  tangent: vec3f,
  phase: f32,
}

struct VertexOut {
  @builtin(position) position: vec4f,
  @location(0) @interpolate(flat, first) baseAlpha: vec4f,
  @location(1) @interpolate(flat, first) creaseColor: vec3f,
  @location(2) localCoord: vec2f,
}

@group(0) @binding(0) var<uniform> view: ViewParams;

fn hashU32(value: u32) -> u32 {
  var state = value * 747796405u + 2891336453u;
  let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
  return (word >> 22u) ^ word;
}

fn unitFloat(value: u32) -> f32 {
  return f32(hashU32(value)) * (1.0 / 4294967296.0);
}

fn safeNormalize(value: vec3f) -> vec3f {
  return value / max(length(value), 0.0001);
}

fn safeNormalize2(value: vec2f) -> vec2f {
  return value / max(length(value), 0.0001);
}

fn cubic(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
  let oneMinusT = 1.0 - t;
  return oneMinusT * oneMinusT * oneMinusT * p0
    + 3.0 * oneMinusT * oneMinusT * t * p1
    + 3.0 * oneMinusT * t * t * p2
    + t * t * t * p3;
}

fn cubicDerivative(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
  let oneMinusT = 1.0 - t;
  return 3.0 * oneMinusT * oneMinusT * (p1 - p0)
    + 6.0 * oneMinusT * t * (p2 - p1)
    + 3.0 * t * t * (p3 - p2);
}

fn sideArc(phase: f32) -> f32 {
  let lookup = array<f32, 32>(
    0.000000, 0.052475, 0.097829, 0.135121, 0.166845, 0.195164, 0.221458, 0.246639,
    0.271368, 0.296184, 0.321577, 0.348019, 0.375973, 0.405832, 0.437746, 0.471327,
    0.505474, 0.538781, 0.570323, 0.599945, 0.628000, 0.655048, 0.681734, 0.708795,
    0.737169, 0.768244, 0.804295, 0.848010, 0.894805, 0.935083, 0.969270, 1.000000,
  );
  let scaled = clamp(phase, 0.0, 0.999999) * 31.0;
  let index = min(u32(floor(scaled)), 30u);
  return mix(lookup[index], lookup[index + 1u], fract(scaled));
}

fn fullArc(phase: f32) -> f32 {
  let lookup = array<f32, 32>(
    0.000000, 0.028092, 0.055939, 0.083892, 0.112291, 0.141449, 0.171637, 0.203033,
    0.235650, 0.269282, 0.303537, 0.337982, 0.372308, 0.406392, 0.440263, 0.474026,
    0.507794, 0.541636, 0.575553, 0.609470, 0.643257, 0.676761, 0.709855, 0.742465,
    0.774594, 0.806319, 0.837790, 0.869218, 0.900862, 0.933020, 0.965991, 1.000000,
  );
  let scaled = clamp(phase, 0.0, 0.999999) * 31.0;
  let index = min(u32(floor(scaled)), 30u);
  return mix(lookup[index], lookup[index + 1u], fract(scaled));
}

fn centerArc(phase: f32) -> f32 {
  let lookup = array<f32, 32>(
    0.000000, 0.028692, 0.059794, 0.096620, 0.140315, 0.179839, 0.212476, 0.241834,
    0.270282, 0.299332, 0.329968, 0.362347, 0.395341, 0.427241, 0.457283, 0.485900,
    0.514192, 0.543773, 0.577230, 0.618093, 0.661144, 0.696770, 0.727388, 0.756064,
    0.784657, 0.814415, 0.845979, 0.878880, 0.911508, 0.942489, 0.971712, 1.000000,
  );
  let scaled = clamp(phase, 0.0, 0.999999) * 31.0;
  let index = min(u32(floor(scaled)), 30u);
  return mix(lookup[index], lookup[index + 1u], fract(scaled));
}

fn mobileArc(phase: f32) -> f32 {
  let lookup = array<f32, 32>(
    0.000000, 0.028885, 0.057970, 0.087431, 0.117400, 0.147935, 0.179017, 0.210560,
    0.242467, 0.274689, 0.307272, 0.340367, 0.374193, 0.408970, 0.444794, 0.481483,
    0.518517, 0.555206, 0.591030, 0.625807, 0.659633, 0.692728, 0.725311, 0.757533,
    0.789440, 0.820983, 0.852065, 0.882600, 0.912569, 0.942030, 0.971115, 1.000000,
  );
  let scaled = clamp(phase, 0.0, 0.999999) * 31.0;
  let index = min(u32(floor(scaled)), 30u);
  return mix(lookup[index], lookup[index + 1u], fract(scaled));
}

fn sidePath(seedPhase: f32, distance: f32, aspect: f32, mirror: f32) -> PathSample {
  let pi = 3.14159265359;
  let pathLength = 2.65 + 0.61 * aspect + 0.09 * aspect * aspect;
  let phase = fract(seedPhase + distance / pathLength);
  let t = sideArc(phase);
  let x = cubic(1.24, 1.02, -0.28, 0.12, t) + sin(t * pi * 4.0 + 0.34) * 0.055;
  let y = cubic(1.38, 0.72, -0.56, -1.38, t) + sin(t * pi * 2.0 - 0.6) * 0.04;
  let z = sin(t * pi * 3.0) * 0.18;
  let derivative = vec3f(
    mirror * aspect * (
      cubicDerivative(1.24, 1.02, -0.28, 0.12, t)
        + cos(t * pi * 4.0 + 0.34) * pi * 4.0 * 0.055
    ),
    cubicDerivative(1.38, 0.72, -0.56, -1.38, t)
      + cos(t * pi * 2.0 - 0.6) * pi * 2.0 * 0.04,
    cos(t * pi * 3.0) * pi * 3.0 * 0.18,
  );
  var sample: PathSample;
  sample.position = vec3f(mirror * aspect * x, y, z);
  sample.tangent = safeNormalize(derivative);
  sample.phase = phase;
  return sample;
}

fn centerPath(seedPhase: f32, distance: f32, aspect: f32) -> PathSample {
  let pi = 3.14159265359;
  let pathLength = 2.3 + 2.0 * aspect + 0.35 * aspect * aspect;
  let phase = fract(seedPhase + distance / pathLength);
  let t = centerArc(phase);
  let angle = mix(-0.25 * pi, 1.75 * pi, t);
  let angleDerivative = 2.0 * pi;
  let radius = 0.72 + sin(t * pi * 4.0) * 0.12;
  let radiusDerivative = cos(t * pi * 4.0) * pi * 4.0 * 0.12;
  let derivative = vec3f(
    aspect * (
      -sin(angle) * angleDerivative * radius
        + cos(angle) * radiusDerivative
    ),
    cos(angle) * angleDerivative * radius
      + sin(angle) * radiusDerivative,
    cos(t * pi * 2.0) * pi * 2.0 * 0.16,
  );
  var sample: PathSample;
  sample.position = vec3f(
    cos(angle) * radius * aspect,
    sin(angle) * radius,
    sin(t * pi * 2.0) * 0.16,
  );
  sample.tangent = safeNormalize(derivative);
  sample.phase = phase;
  return sample;
}

fn fullPath(seedPhase: f32, distance: f32, aspect: f32) -> PathSample {
  let pi = 3.14159265359;
  let pathWidth = 2.44 * aspect;
  let pathLength = sqrt(pathWidth * pathWidth + 5.0);
  let phase = fract(seedPhase + distance / pathLength);
  let t = fullArc(phase);
  let derivative = vec3f(
    aspect * 2.44,
    cos((t * 1.72 - 0.2) * pi) * 1.72 * pi * 0.54
      + cos(t * pi * 3.0) * pi * 3.0 * 0.12,
    -sin(t * pi * 2.0 - 0.7) * pi * 2.0 * 0.22,
  );
  var sample: PathSample;
  sample.position = vec3f(
    mix(-aspect * 1.22, aspect * 1.22, t),
    sin((t * 1.72 - 0.2) * pi) * 0.54 + sin(t * pi * 3.0) * 0.12,
    cos(t * pi * 2.0 - 0.7) * 0.22,
  );
  sample.tangent = safeNormalize(derivative);
  sample.phase = phase;
  return sample;
}

fn mobilePath(seedPhase: f32, distance: f32, aspect: f32) -> PathSample {
  let pi = 3.14159265359;
  let pathWidth = 2.56 * aspect;
  let pathLength = sqrt(pathWidth * pathWidth + 1.0);
  let phase = fract(seedPhase + distance / pathLength);
  let t = mobileArc(phase);
  let derivative = vec3f(
    aspect * 2.56,
    cos(t * pi) * pi * 0.28 + cos(t * pi * 3.0) * pi * 3.0 * 0.06,
    -sin(t * pi * 2.0) * pi * 2.0 * 0.16,
  );
  var sample: PathSample;
  sample.position = vec3f(
    mix(-aspect * 1.28, aspect * 1.28, t),
    -0.86 + sin(t * pi) * 0.28 + sin(t * pi * 3.0) * 0.06,
    cos(t * pi * 2.0) * 0.16,
  );
  sample.tangent = safeNormalize(derivative);
  sample.phase = phase;
  return sample;
}

fn weightedPath(seedPhase: f32, phaseOffset: f32, aspect: f32, weights: vec4f) -> PathSample {
  // Every placement samples the same point along the stream, including its wrap seam.
  let phase = fract(seedPhase + phaseOffset);
  var result: PathSample;
  result.position = vec3f(0.0);
  result.tangent = vec3f(0.0);
  result.phase = phase;

  if (aspect < 0.82) {
    let compactWeight = weights.x + weights.y + weights.z;
    if (compactWeight > 0.0001) {
      let compact = mobilePath(phase, 0.0, aspect);
      result.position += compact.position * compactWeight;
      result.tangent += compact.tangent * compactWeight;
    }
    if (weights.w > 0.0001) {
      let wide = fullPath(phase, 0.0, aspect);
      result.position += wide.position * weights.w;
      result.tangent += wide.tangent * weights.w;
    }
  } else {
    if (weights.x > 0.0001) {
      let right = sidePath(phase, 0.0, aspect, 1.0);
      result.position += right.position * weights.x;
      result.tangent += right.tangent * weights.x;
    }
    if (weights.y > 0.0001) {
      let left = sidePath(phase, 0.0, aspect, -1.0);
      result.position += left.position * weights.y;
      result.tangent += left.tangent * weights.y;
    }
    if (weights.z > 0.0001) {
      let center = centerPath(phase, 0.0, aspect);
      result.position += center.position * weights.z;
      result.tangent += center.tangent * weights.z;
    }
    if (weights.w > 0.0001) {
      let wide = fullPath(phase, 0.0, aspect);
      result.position += wide.position * weights.w;
      result.tangent += wide.tangent * weights.w;
    }
  }

  result.tangent = safeNormalize(result.tangent + vec3f(0.0001, 0.0, 0.0));
  return result;
}

fn pointerField(delta: vec2f, radius: f32, flow: vec2f, depth: f32) -> vec2f {
  // A curved Gaussian follows the flow, with a long, boundary-free tail.
  let offset = delta / max(radius, 0.001);
  let along = dot(offset, flow);
  let across = dot(offset, vec2f(-flow.y, flow.x));
  let alongSquared = along * along;
  let layer = depth * inverseSqrt(1.0 + depth * depth);
  let bend = (0.22 * alongSquared + 0.12 * layer * along) / (1.0 + alongSquared);
  let curvedAcross = (across + bend) / (1.0 + layer * 0.18);
  let falloff = exp(-0.28 * alongSquared - 1.2 * curvedAcross * curvedAcross);
  return offset * falloff;
}

fn rippleWave(age: f32) -> f32 {
  if (age <= 0.0 || age >= 1.8) { return 0.0; }
  let attack = smoothstep(0.0, 0.14, age);
  let release = 1.0 - smoothstep(1.4, 1.8, age);
  return sin(age * 10.0) * exp(-age * 3.2) * attack * release;
}

fn rippleDisplacement(position: vec3f, pulse: vec4f) -> vec4f {
  if (pulse.w <= 0.0001) { return vec4f(0.0); }
  let perspective = 1.0 / max(0.62, 1.0 - position.z * 0.34);
  let delta = (position.xy * perspective - pulse.xy) * view.viewport.z;
  let distance = sqrt(dot(delta, delta) + 0.0016) - 0.04;
  let wave = rippleWave(pulse.z - distance / 4.2) * pulse.w;
  let radial = delta / (distance + 0.12);
  return vec4f(radial * wave * 0.28, wave * 0.12, abs(wave));
}

fn shardVertex(index: u32) -> vec3f {
  let fold = 0.34;
  let vertices = array<vec3f, 6>(
    vec3f(0.0, 1.0, fold),
    vec3f(-0.72, 0.0, 0.0),
    vec3f(0.0, -1.0, fold),
    vec3f(0.0, 1.0, fold),
    vec3f(0.0, -1.0, fold),
    vec3f(0.72, 0.0, 0.0)
  );
  return vertices[index % 6u];
}

fn softbox(direction: vec3f, center: vec2f, size: vec2f) -> f32 {
  let q = abs((direction.xy - center) / size);
  let q2 = q * q;
  let q4 = q2 * q2;
  return exp(-(q4.x + q4.y));
}

fn aces(color: vec3f) -> vec3f {
  let a = 2.51;
  let b = 0.03;
  let c = 2.43;
  let d = 0.59;
  let e = 0.14;
  return clamp((color * (a * color + b)) / (color * (c * color + d) + e), vec3f(0.0), vec3f(1.0));
}

@vertex
fn vs_main(
  @builtin(vertex_index) vertexIndex: u32,
  @builtin(instance_index) instanceIndex: u32,
) -> VertexOut {
  let seedPhase = unitFloat(instanceIndex * 1664525u + 1013904223u);
  let seedLane = unitFloat(instanceIndex * 2246822519u + 3266489917u);
  let seedDepth = unitFloat(instanceIndex * 668265263u + 374761393u);
  let seedScale = unitFloat(instanceIndex * 1597334677u + 3812015801u);
  let aspect = view.viewport.x;
  let path = weightedPath(seedPhase, view.transport.x, aspect, view.composition);
  var direction = path.tangent;
  var planarNormal = safeNormalize2(vec2f(-direction.y, direction.x));

  let signedLane = seedLane * 2.0 - 1.0;
  let lane = sign(signedLane) * pow(abs(signedLane), 0.72);
  let widthProfile = 0.46 + pow(max(sin(path.phase * 3.14159265359), 0.0), 0.72) * 0.54;
  let looseSeed = unitFloat(instanceIndex * 3266489917u + 668265263u);
  let loose = smoothstep(0.92, 1.0, looseSeed);
  let flowWave = sin(path.phase * 37.6991118431 + seedDepth * 12.0);
  let laneWidth = (lane * 0.56 + flowWave * 0.055 * view.shape.z) * view.shape.x
    * widthProfile * (1.0 + loose * 0.72);
  let depthLane = (seedDepth * 2.0 - 1.0) * view.shape.y
    + cos(path.phase * 31.4159265359 + seedLane * 8.0) * 0.06 * view.shape.z;
  var renderPosition = path.position + vec3f(planarNormal * laneWidth, depthLane);

  if (view.formation.y + view.formation.z > 0.00001) {
    let center = vec2f((view.composition.x - view.composition.y) * aspect * 0.56, 0.0);
    var formedPosition = renderPosition * view.formation.x;
    var formedDirection = direction * view.formation.x;
    if (view.formation.y > 0.00001) {
      let radius = 0.16 + sqrt(seedLane) * 0.74 * (0.45 + view.shape.x * 0.55);
      // Constant tangential travel speed; inner rings turn faster without speeding up.
      let angle = seedPhase * 6.28318530718 + view.viewport.w / radius;
      let radial = vec2f(cos(angle), sin(angle));
      let position = vec3f(center + radial * radius, (seedDepth - 0.5) * view.shape.y * 0.65 + radial.y * 0.2);
      formedPosition += position * view.formation.y;
      formedDirection += safeNormalize(vec3f(-radial.y, radial.x, radial.x * 0.2)) * view.formation.y;
    }
    if (view.formation.z > 0.00001) {
      let phase = fract(seedPhase + view.viewport.w / (aspect * 3.0 + 2.0));
      let angle = phase * 6.28318530718;
      let ribbonWidth = (seedLane - 0.5) * 0.54 * view.shape.x;
      let position = vec3f(
        mix(-aspect * 1.35, aspect * 1.35, phase) + center.x * 0.5,
        sin(angle) * 0.42 + cos(angle * 2.0) * ribbonWidth,
        (cos(angle) * 0.35 + sin(angle * 2.0) * ribbonWidth + (seedDepth - 0.5) * 0.12) * view.shape.y
      );
      let tangent = safeNormalize(vec3f(aspect * 2.7, cos(angle) * 2.638938, -sin(angle) * 2.199115 * view.shape.y));
      formedPosition += position * view.formation.z;
      formedDirection += tangent * view.formation.z;
    }
    renderPosition = formedPosition;
    direction = safeNormalize(formedDirection + vec3f(0.0, 0.0, 0.02 * view.formation.x * (1.0 - view.formation.x)));
    planarNormal = safeNormalize2(vec2f(-direction.y, direction.x));
  }

  if (abs(view.pointer.w) > 0.0001) {
    let field = pointerField(
      view.pointer.xy - renderPosition.xy,
      view.pointer.z,
      vec2f(planarNormal.y, -planarNormal.x),
      renderPosition.z,
    );
    let lateral = field - direction.xy * dot(field, direction.xy);
    renderPosition += vec3f(lateral * view.pointer.w * 0.36, 0.0);
    direction = safeNormalize(vec3f(direction.xy + lateral * view.pointer.w * 0.65, direction.z));
  }

  if (view.gather.z > 0.00001) {
    // A Gaussian cloud has a dense center and soft outskirts, never a ring or a hard outline.
    let relative = (renderPosition.xy - view.gather.xy) * view.viewport.z;
    let reach = length(relative);
    let radius = sqrt(-2.0 * log(max(seedLane, 0.0001)));
    let angle = seedPhase * 6.28318530718 + view.gather.w * (0.3 + seedDepth * 0.18);
    let orbit = vec2f(cos(angle), sin(angle));
    let layer = seedDepth * 6.28318530718;
    let drift = vec2f(sin(layer + view.gather.w * 0.22), cos(layer * 1.7 - view.gather.w * 0.18)) * 0.055;
    let cloud = orbit * radius * vec2f(0.2, 0.16) + drift;
    let cluster = vec3f(view.gather.xy + cloud / view.viewport.z, (seedDepth - 0.5) * 0.42);
    // Distant layers arrive later; there is no single closing boundary.
    let amount = pow(view.gather.z, 1.0 + seedDepth * 0.65 + min(reach, 4.0) * 0.12);
    let curledDirection = safeNormalize(vec3f(-orbit.y, orbit.x, sin(layer) * 0.35));
    renderPosition = mix(renderPosition, cluster, amount);
    direction = safeNormalize(mix(direction, curledDirection, amount));
  }

  var rippleLight = 0.0;
  if (view.shock.w + view.shockB.w + view.shockC.w + view.shockD.w > 0.0001) {
    let displacement = rippleDisplacement(renderPosition, view.shock)
      + rippleDisplacement(renderPosition, view.shockB)
      + rippleDisplacement(renderPosition, view.shockC)
      + rippleDisplacement(renderPosition, view.shockD);
    rippleLight = min(displacement.w, 1.5);
    if (dot(displacement.xyz, displacement.xyz) > 0.0) {
      renderPosition += displacement.xyz;
      direction = safeNormalize(direction + displacement.xyz * 0.7);
    }
  }

  let shapeLocal = shardVertex(vertexIndex);
  var local = shapeLocal;
  local.x *= mix(0.72, 1.08, seedLane);
  local.y *= mix(0.82, 1.12, seedDepth);
  local.x += (seedDepth - 0.5) * (1.0 - abs(local.y)) * 0.16;

  var side = cross(vec3f(0.0, 0.0, 1.0), direction);
  let sideLengthSquared = dot(side, side);
  if (sideLengthSquared > 0.0001) {
    side *= inverseSqrt(sideLengthSquared);
  } else {
    side = vec3f(1.0, 0.0, 0.0);
  }
  let facing = cross(direction, side);
  let rollDirection = mix(-1.5, 1.7, seedDepth);
  let roll = seedLane * 6.28318530718 + view.viewport.w * rollDirection * view.effects.x * 2.4;
  let rollSin = sin(roll);
  let rollCos = cos(roll);
  let bankedSide = side * rollCos + facing * rollSin;
  let bankedFacing = facing * rollCos - side * rollSin;
  let depthScale = mix(0.56, 1.58, clamp(renderPosition.z * 0.62 + 0.5, 0.0, 1.0));
  let scaleShape = 0.46 + seedScale * 0.58 + pow(seedScale, 12.0) * 1.55;
  let size = view.viewport.y * scaleShape * depthScale * (1.0 - view.gather.z * 0.3);
  let width = size * 0.72;
  let lengthScale = size * 1.26 * view.effects.z;
  let world = renderPosition
    + direction * local.y * lengthScale
    + bankedSide * local.x * width
    + bankedFacing * local.z * width;

  let perspective = 1.0 / max(0.62, 1.0 - world.z * 0.34);
  let ndc = world.xy * view.viewport.z / vec2f(aspect, 1.0) * perspective;
  let depth = clamp(0.56 - world.z * 0.24, 0.03, 0.97);
  let triangle = vertexIndex / 3u;
  let corner = vertexIndex % 3u;
  var mapped = vec3f(0.0);
  var mappedCrease = vec3f(0.0);
  var shardAlpha = 0.0;

  if (corner == 0u) {
    let facetSide = select(-1.0, 1.0, triangle == 1u);
    let localNormal = vec3f(facetSide * 0.394903, 0.0, 0.918723);
    let normal = bankedSide * localNormal.x + bankedFacing * localNormal.z;
    let viewDirection = normalize(vec3f(-renderPosition.xy * 0.08, 1.0));
    let pointerShift = vec2f(view.light.w, view.shape.w);
    let keyDirection = view.light.xyz;
    let halfDirection = normalize(keyDirection + viewDirection);
    let roughness = clamp(view.material.x, 0.04, 0.96);
    let materialKind = view.material.y;
    let glow = view.material.w;
    let reflection = reflect(-viewDirection, normal);
    let broad = softbox(
      reflection,
      vec2f(-0.34, 0.28) + pointerShift * 0.36,
      vec2f(0.52, 0.22) + roughness * 0.3,
    );
    let strip = softbox(
      reflection,
      vec2f(0.48, -0.08) - pointerShift * 0.2,
      vec2f(0.12, 0.72),
    );
    let diffuse = max(dot(normal, keyDirection), 0.0);
    let specularPower = mix(92.0, 9.0, roughness);
    let specular = pow(max(dot(normal, halfDirection), 0.0), specularPower);
    let fresnelBase = 1.0 - max(dot(normal, viewDirection), 0.0);
    let fresnelSquared = fresnelBase * fresnelBase;
    let fresnel = fresnelSquared * fresnelSquared;
    let facet = mix(0.76, 1.0, smoothstep(-0.08, 0.08, normal.x));
    let depthFog = smoothstep(-0.68, 0.58, renderPosition.z);
    let depthTint = mix(view.accentColor.rgb * 0.52, view.baseColor.rgb, depthFog);
    var color = depthTint * (0.1 + diffuse * 0.3) * facet;
    color += view.highlightColor.rgb * (broad * mix(0.3, 0.86, 1.0 - roughness)) * (1.0 + glow * 0.14);
    color += view.accentColor.rgb * strip * (0.12 + fresnel * 0.42);
    color += view.highlightColor.rgb * specular * mix(0.82, 1.0, seedDepth);
    color += mix(view.baseColor.rgb, view.accentColor.rgb, seedLane) * fresnel * (0.15 + glow * 0.16);
    color += view.accentColor.rgb * (broad * 0.045 + fresnel * 0.075) * glow;
    var creaseColor = color + view.highlightColor.rgb * (0.08 + specular * 0.22);

    if (materialKind > 0.5 && materialKind < 1.5) {
      let materialLight = view.highlightColor.rgb * (broad + specular) * 0.32;
      color = color * 1.1 + materialLight;
      creaseColor = creaseColor * 1.1 + materialLight;
    } else if (materialKind >= 1.5) {
      let satinColor = view.baseColor.rgb * (0.46 + diffuse * 0.46);
      color = mix(color, satinColor, 0.56);
      creaseColor = mix(creaseColor, satinColor, 0.56);
    }

    // A light page acts as a broad fill light, keeping shaded facets in the chosen palette.
    let fill = mix(view.accentColor.rgb, view.baseColor.rgb, depthFog)
      * (0.38 + diffuse * 0.12) * facet * view.environment.x;
    color += fill;
    creaseColor += fill;

    // Reuse the displacement wave, so the accent catches each facet as the ripple arrives.
    let pulseColor = mix(view.accentColor.rgb, view.highlightColor.rgb, 0.18);
    color += pulseColor * rippleLight * (0.85 + fresnel * 0.45);
    creaseColor += pulseColor * rippleLight * 1.35;

    let fog = mix(0.42, 1.0, depthFog);
    let exposure = fog * view.material.z * view.effects.w;
    mapped = aces(color * exposure);
    mappedCrease = aces(creaseColor * exposure);
    shardAlpha = mix(0.58, 0.97, depthFog);
    // Open path endpoints can cross the viewport while morphing. Taper only that moving seam.
    let seam = smoothstep(0.0, 0.035, path.phase) * (1.0 - smoothstep(0.965, 1.0, path.phase));
    shardAlpha *= mix(1.0, seam, view.transport.y * view.formation.x * (1.0 - view.gather.z));
  }

  var out: VertexOut;
  out.position = vec4f(ndc, depth, 1.0);
  out.baseAlpha = vec4f(mapped, shardAlpha);
  out.creaseColor = mappedCrease - mapped;
  out.localCoord = shapeLocal.xy;
  return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4f {
  let crease = (1.0 - smoothstep(0.015, 0.11, abs(in.localCoord.x)))
    * (1.0 - smoothstep(0.78, 1.0, abs(in.localCoord.y)));
  var coverage = 1.0;
  if (view.effects.y > 0.001) {
    let diamondDistance = 1.0 - abs(in.localCoord.y) - abs(in.localCoord.x) / 0.72;
    let edgeWidth = max(fwidth(diamondDistance) * view.effects.y, 0.0001);
    coverage = smoothstep(0.0, edgeWidth, diamondDistance);
  }
  let mapped = in.baseAlpha.rgb + in.creaseColor * crease;
  let coveredAlpha = in.baseAlpha.a * coverage;
  return vec4f(mapped * coveredAlpha, coveredAlpha);
}
