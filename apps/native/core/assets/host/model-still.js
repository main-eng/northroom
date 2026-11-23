// The drawing of 3D models' stills (ADR-0003), as plain JavaScript inside the still page's
// module (`model-still.html`): drawStill(bytes) resolves with the PNG as a data URL. The page
// draws one model after another, so what every still shares (the renderer, the studio, the
// lights and the camera) is made once, at the first, and each model's own is let go after its
// still. THREE is the page host's pinned three.js.
const SIZE = 1024,
  SIDE = 640,
  FILL = 0.7,
  MARGIN = 0.86,
  TURN = -35,
  TILT = -22,
  DEG = Math.PI / 180;
// The editor's built-in studio (studio-environment.ts), before normalisation.
const STUDIO = {
  floor: [0.034, 0.033, 0.032],
  ceilingAtHorizon: [0.17, 0.17, 0.175],
  ceilingAtZenith: [0.62, 0.63, 0.66],
  floorToCeiling: [-0.03, 0.05],
  horizonStrip: { elevation: 0.012, width: 0.022, radiance: [1.05, 0.88, 0.7] },
  coolWash: { elevation: -0.07, width: 0.06, radiance: [0.075, 0.088, 0.105] },
  floorBounce: { azimuth: 0, elevation: -0.42, width: 0.38, radiance: [0.13, 0.125, 0.115] },
  key: {
    azimuth: -0.22,
    elevation: 0.44,
    halfWidth: 0.6,
    halfHeight: 0.22,
    cornerRadius: 0.07,
    radiance: [5.2, 5.2, 5.1],
  },
  keyGlow: { width: 0.09, radiance: [0.42, 0.34, 0.25] },
  rim: {
    azimuth: 2.45,
    elevation: 0.28,
    halfWidth: 0.05,
    halfHeight: 0.55,
    cornerRadius: 0.03,
    radiance: [2.6, 2.7, 2.85],
  },
  fill: {
    azimuth: -2.25,
    elevation: 0.2,
    halfWidth: 0.32,
    halfHeight: 0.3,
    cornerRadius: 0.08,
    radiance: [0.85, 0.85, 0.85],
  },
  flags: { azimuth: 1.45, halfWidth: 0.36, elevationRange: [-0.25, 1.0], shade: 0.08 },
  edgeSoftness: 0.02,
};
const smoothstep = (a, b, x) => {
  const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
  return t * t * (3 - 2 * t);
};
const gaussian = (offset, width) => Math.exp(-((offset / width) ** 2));
const azimuthDelta = (to, from) => {
  const d = (to - from) % (2 * Math.PI);
  return d > Math.PI ? d - 2 * Math.PI : d <= -Math.PI ? d + 2 * Math.PI : d;
};
const panelDistance = (p, az, el) => {
  const qx = Math.abs(azimuthDelta(az, p.azimuth)) * Math.cos(el) - p.halfWidth + p.cornerRadius;
  const qy = Math.abs(el - p.elevation) - p.halfHeight + p.cornerRadius;
  return (
    Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - p.cornerRadius
  );
};
const coverage = (p, az, el) =>
  1 - smoothstep(-STUDIO.edgeSoftness, STUDIO.edgeSoftness, panelDistance(p, az, el));
function studio(x, y, z) {
  const el = Math.asin(Math.min(1, Math.max(-1, y))),
    az = Math.atan2(x, z);
  const ceilingBlend = smoothstep(0, 1, y),
    sky = smoothstep(STUDIO.floorToCeiling[0], STUDIO.floorToCeiling[1], el);
  const strip = gaussian(el - STUDIO.horizonStrip.elevation, STUDIO.horizonStrip.width);
  const wash = gaussian(el - STUDIO.coolWash.elevation, STUDIO.coolWash.width);
  const bounce =
    gaussian(azimuthDelta(az, 0), STUDIO.floorBounce.width) *
    gaussian(el - STUDIO.floorBounce.elevation, STUDIO.floorBounce.width);
  const e = STUDIO.edgeSoftness,
    f = STUDIO.flags;
  const flagged =
    smoothstep(f.halfWidth + e, f.halfWidth - e, Math.abs(Math.abs(az) - f.azimuth)) *
    smoothstep(f.elevationRange[0] - e, f.elevationRange[0] + e, el) *
    smoothstep(f.elevationRange[1] + e, f.elevationRange[1] - e, el);
  const flagShade = 1 - flagged * (1 - f.shade);
  const keyDistance = panelDistance(STUDIO.key, az, el),
    key = coverage(STUDIO.key, az, el);
  const glow = keyDistance > 0 ? Math.exp(-keyDistance / STUDIO.keyGlow.width) : 0;
  const rim = coverage(STUDIO.rim, az, el),
    fill = coverage(STUDIO.fill, az, el);
  return [0, 1, 2].map((c) => {
    const ceiling =
      STUDIO.ceilingAtHorizon[c] +
      (STUDIO.ceilingAtZenith[c] - STUDIO.ceilingAtHorizon[c]) * ceilingBlend;
    const room =
      STUDIO.floor[c] +
      (ceiling - STUDIO.floor[c]) * sky +
      STUDIO.horizonStrip.radiance[c] * strip +
      STUDIO.coolWash.radiance[c] * wash * (1 - sky) +
      STUDIO.floorBounce.radiance[c] * bounce * (1 - sky);
    return (
      room * flagShade +
      STUDIO.key.radiance[c] * key +
      STUDIO.keyGlow.radiance[c] * glow * (1 - key) +
      STUDIO.rim.radiance[c] * rim +
      STUDIO.fill.radiance[c] * fill
    );
  });
}
// three.js's equirectangular image: column u is atan2(z, x), row v the elevation from
// straight down. Each channel's mean over the sphere is made one, as the editor's is.
function studioImage(width, height) {
  const texels = new Float32Array(width * height * 4),
    sums = [0, 0, 0];
  let weight = 0;
  for (let row = 0; row < height; row++) {
    const el = ((row + 0.5) / height - 0.5) * Math.PI,
      cos = Math.cos(el);
    for (let column = 0; column < width; column++) {
      const a = ((column + 0.5) / width - 0.5) * 2 * Math.PI;
      const r = studio(Math.cos(a) * cos, Math.sin(el), Math.sin(a) * cos),
        o = (row * width + column) * 4;
      for (let c = 0; c < 3; c++) {
        texels[o + c] = r[c];
        sums[c] += r[c] * cos;
      }
      texels[o + 3] = 1;
      weight += cos;
    }
  }
  for (let i = 0; i < texels.length; i += 4)
    for (let c = 0; c < 3; c++) texels[i + c] /= sums[c] / weight;
  const halves = new Uint16Array(texels.length);
  for (let i = 0; i < texels.length; i++) halves[i] = THREE.DataUtils.toHalfFloat(texels[i]);
  const texture = new THREE.DataTexture(
    halves,
    width,
    height,
    THREE.RGBAFormat,
    THREE.HalfFloatType,
  );
  texture.colorSpace = THREE.LinearSRGBColorSpace;
  texture.mapping = THREE.EquirectangularReflectionMapping;
  texture.magFilter = texture.minFilter = THREE.LinearFilter;
  texture.needsUpdate = true;
  return texture;
}
// The Studio rig's ambience takes no diffuse light from the image (only its
// reflections and their multiple scattering), and adds 0.14 plus 0.10 toward the sky,
// as irradiance (three.js divides it by pi). The camera is not turned, so the view's
// normal is the world's.
THREE.ShaderChunk.lights_physical_pars_fragment =
  THREE.ShaderChunk.lights_physical_pars_fragment.replace(
    "vec3 indirectDiffuse = diffuse * cosineWeightedIrradiance;",
    "vec3 indirectDiffuse = vec3( 0.0 );",
  );
THREE.ShaderChunk.lights_fragment_begin = THREE.ShaderChunk.lights_fragment_begin.replace(
  "vec3 irradiance = getAmbientLightIrradiance( ambientLightColor );",
  "vec3 irradiance = getAmbientLightIrradiance( ambientLightColor ) + vec3( PI * ( 0.14 + 0.10 * max( normal.y, 0.0 ) ) );",
);
// What every still shares: made at the first, kept for the next.
let shared = null;
function rig() {
  if (shared) return shared;
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = SIZE;
  const renderer = new THREE.WebGLRenderer({
    canvas,
    antialias: true,
    alpha: true,
    premultipliedAlpha: true,
    preserveDrawingBuffer: true,
  });
  renderer.setPixelRatio(1);
  renderer.setSize(SIZE, SIZE, false);
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.toneMapping = THREE.NoToneMapping;
  renderer.setClearColor(0x000000, 0);
  const prefilter = new THREE.PMREMGenerator(renderer);
  const environment = prefilter.fromEquirectangular(studioImage(512, 256)).texture;
  prefilter.dispose();
  const lights = [
    [
      [-0.5, 0.8, 1.0],
      [3.0, 3.1, 3.4],
    ],
    [
      [0.7, 0.1, 0.6],
      [1.5, 1.4, 1.3],
    ],
    [
      [0.0, 0.6, -1.0],
      [2.0, 2.0, 2.0],
    ],
  ].map(([direction, radiance]) => {
    const light = new THREE.DirectionalLight(
      new THREE.Color().setRGB(...radiance, THREE.LinearSRGBColorSpace),
      1,
    );
    light.position.set(...direction).normalize();
    return light;
  });
  const camera = new THREE.PerspectiveCamera(60, 1, 0.1, 10000);
  camera.position.set(0, 0, SIZE / 2 / Math.tan(30 * DEG));
  camera.lookAt(0, 0, 0);
  const read = document.createElement("canvas");
  read.width = read.height = SIZE;
  const picture = document.createElement("canvas");
  picture.width = picture.height = SIDE;
  shared = { canvas, renderer, environment, lights, camera, read, picture };
  return shared;
}
// Lets go of what a model's still alone used: its geometry, materials and textures.
function release(model) {
  model.traverse((node) => {
    if (!node.isMesh) return;
    node.geometry.dispose();
    for (const material of [].concat(node.material)) {
      for (const value of Object.values(material)) if (value && value.isTexture) value.dispose();
      material.dispose();
    }
  });
}
// The square around what the frame covers, with the still's margin; none for an empty frame.
function cropped(read) {
  const pixels = read
    .getContext("2d", { willReadFrequently: true })
    .getImageData(0, 0, SIZE, SIZE).data;
  let minX = SIZE,
    minY = SIZE,
    maxX = -1,
    maxY = -1;
  for (let y = 0; y < SIZE; y++)
    for (let x = 0; x < SIZE; x++) {
      if (pixels[(y * SIZE + x) * 4 + 3] <= 8) continue;
      if (x < minX) minX = x;
      if (x > maxX) maxX = x;
      if (y < minY) minY = y;
      if (y > maxY) maxY = y;
    }
  if (maxX < 0) return null;
  const side = Math.max(maxX - minX + 1, maxY - minY + 1) / MARGIN;
  return [(minX + maxX + 1) / 2 - side / 2, (minY + maxY + 1) / 2 - side / 2, side];
}
globalThis.drawStill = async function drawStill(bytes) {
  const gltf = await new GLTFLoader().parseAsync(bytes, "");
  const model = gltf.scene;
  try {
    return framed(model);
  } finally {
    release(model);
  }
};
// The model framed, lit and cut to its square, as a PNG data URL.
function framed(model) {
  model.updateMatrixWorld(true);
  const box = new THREE.Box3().setFromObject(model, true);
  if (box.isEmpty()) throw new Error("the model has no vertices");
  const size = box.getSize(new THREE.Vector3()),
    centre = box.getCenter(new THREE.Vector3());
  const largest = Math.max(size.x, size.y, size.z);
  const w = Math.max(size.x, largest * 0.01),
    h = Math.max(size.y, largest * 0.01),
    d = size.z;
  const turn = TURN * DEG,
    tilt = TILT * DEG;
  const across = w * Math.abs(Math.cos(turn)) + d * Math.abs(Math.sin(turn));
  const deep = w * Math.abs(Math.sin(turn)) + d * Math.abs(Math.cos(turn));
  const up = h * Math.abs(Math.cos(tilt)) + deep * Math.abs(Math.sin(tilt));
  const scale = (FILL * SIZE) / Math.max(across, up);
  model.position.sub(centre);
  const pivot = new THREE.Group();
  pivot.add(model);
  pivot.scale.setScalar(scale);
  pivot.rotation.set(-tilt, turn, 0, "XYZ");
  model.traverse((node) => {
    if (!node.isMesh) return;
    for (const material of [].concat(node.material)) material.side = THREE.DoubleSide;
  });
  const { canvas, renderer, environment, lights, camera, read, picture } = rig();
  const scene = new THREE.Scene();
  scene.add(pivot, ...lights);
  scene.environment = environment;
  scene.environmentIntensity = 0.2;
  renderer.render(scene, camera);
  const context = read.getContext("2d", { willReadFrequently: true });
  context.clearRect(0, 0, SIZE, SIZE);
  context.drawImage(canvas, 0, 0);
  const square = cropped(read);
  if (!square) throw new Error("the model drew nothing to show");
  const [x, y, side] = square;
  const out = picture.getContext("2d");
  out.clearRect(0, 0, SIDE, SIDE);
  out.imageSmoothingQuality = "high";
  out.drawImage(read, x, y, side, side, 0, 0, SIDE, SIDE);
  return picture.toDataURL("image/png");
}
