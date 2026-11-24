import * as THREE from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';

// ============================================================================
// Constants matching docs/demo/src/mountain_trail.ter (400x400x40m, skirt=1)
// ============================================================================
const WORLD_W = 400;
const WORLD_H = 40;
const WORLD_D = 400;
const SEA_LEVEL_NORM = 0.10;
// mesher.rs: base_y = max(wy * 0.14, min(wx, wz) * 0.035) = max(5.6, 14.0) = 14.0
const TERRAIN_BASE_Y = 14.0;
const SEA_Y = TERRAIN_BASE_Y + SEA_LEVEL_NORM * WORLD_H; // 18.0m

const CABIN_POS = new THREE.Vector2(0, 6);
const CAR_START = { x: -12, z: 32, yaw: 2.76 };
const FOX_POS = new THREE.Vector2(80, -48);

// 18 curated scenic coin waypoints across the 400x400m island (u,v in world meters)
const COIN_COORDS = [
  [-6, 18],      // 1. Right outside the cabin porch (immediate sightline!)
  [26, 58],      // 2. Meadow bend
  [44, 92],      // 3. River valley approach
  [18, 128],     // 4. Sandy cove east shore
  [-24, 142],    // 5. Cove beach south point
  [-72, 122],    // 6. Southwest headland
  [-96, 56],     // 7. Inside the crater bowl!
  [-116, 12],    // 8. West pine slope
  [-86, -34],    // 9. West meadow pass
  [-48, -66],    // 10. Northern valley gateway
  [-14, -92],    // 11. Alpine amphitheater foothills
  [36, -88],     // 12. High ridge switchback
  [78, -54],     // 13. Red Fox Overlook landmark!
  [118, -26],    // 14. East ridge vista
  [124, 34],     // 15. Eastern pine woods
  [92, 82],      // 16. Southeast headland trail
  [-44, 42],     // 17. Central forest glade
  [-18, -24],    // 18. Behind the mountain cabin
];

// ============================================================================
// DOM Elements
// ============================================================================
const canvas = document.getElementById('game-canvas');
const loadingScreen = document.getElementById('loading-screen');
const loadingText = document.getElementById('loading-text');
const loadingBar = document.getElementById('loading-bar');
const startMenu = document.getElementById('start-menu');
const pauseMenu = document.getElementById('pause-menu');
const victoryMenu = document.getElementById('victory-menu');
const hud = document.getElementById('hud');

const hudCoins = document.getElementById('hud-coins');
const hudTimer = document.getElementById('hud-timer');
const hudDist = document.getElementById('hud-dist');
const compassArrow = document.getElementById('compass-arrow');
const hudSpeed = document.getElementById('hud-speed');
const hudRpm = document.getElementById('hud-rpm');
const toastEl = document.getElementById('toast');
const minimapCanvas = document.getElementById('minimap-canvas');
const minimapCtx = minimapCanvas.getContext('2d');

const btnPlay = document.getElementById('btn-play');
const btnResume = document.getElementById('btn-resume');
const btnResetCar = document.getElementById('btn-reset-car');
const btnRestart = document.getElementById('btn-restart');
const btnFreeroam = document.getElementById('btn-freeroam');
const btnPlayAgain = document.getElementById('btn-play-again');
const btnHudCam = document.getElementById('btn-hud-cam');
const btnHudMute = document.getElementById('btn-hud-mute');
const btnHudPause = document.getElementById('btn-hud-pause');
const victoryStats = document.getElementById('victory-stats');

const volMusicSlider = document.getElementById('vol-music');
const volAmbSlider = document.getElementById('vol-ambience');
const volSfxSlider = document.getElementById('vol-sfx');
const valMusic = document.getElementById('val-music');
const valAmb = document.getElementById('val-ambience');
const valSfx = document.getElementById('val-sfx');

// ============================================================================
// Three.js Renderer, Scene & Lighting
// ============================================================================
const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, powerPreference: 'high-performance' });
renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
renderer.setSize(window.innerWidth, window.innerHeight);
renderer.shadowMap.enabled = true;
renderer.shadowMap.type = THREE.PCFSoftShadowMap;
renderer.toneMapping = THREE.ACESFilmicToneMapping;
renderer.toneMappingExposure = 1.08;

const scene = new THREE.Scene();
scene.background = new THREE.Color(0x9fc2e6);
scene.fog = new THREE.FogExp2(0xa9c7e4, 0.0026);

const camera = new THREE.PerspectiveCamera(58, window.innerWidth / window.innerHeight, 0.2, 750);

// Warm alpine sunlight + crisp sky/ground hemisphere fill
const hemiLight = new THREE.HemisphereLight(0xdff0ff, 0x4b5c3e, 0.85);
scene.add(hemiLight);

const sunLight = new THREE.DirectionalLight(0xfff4dc, 2.15);
sunLight.position.set(-95, 130, 85);
sunLight.castShadow = true;
sunLight.shadow.mapSize.set(2048, 2048);
sunLight.shadow.camera.near = 20;
sunLight.shadow.camera.far = 420;
const shadowExt = 95;
sunLight.shadow.camera.left = -shadowExt;
sunLight.shadow.camera.right = shadowExt;
sunLight.shadow.camera.top = shadowExt;
sunLight.shadow.camera.bottom = -shadowExt;
sunLight.shadow.bias = -0.0005;
scene.add(sunLight);
scene.add(sunLight.target);

// Simple stylized alpine sky dome + distant horizon water extension
{
  const skyGeo = new THREE.SphereGeometry(520, 24, 16);
  const pos = skyGeo.attributes.position;
  const cols = new Float32Array(pos.count * 3);
  const topCol = new THREE.Color(0x3a6ea5);
  const midCol = new THREE.Color(0x9bc2e6);
  const botCol = new THREE.Color(0xdbe8f5);
  const tmp = new THREE.Color();
  for (let i = 0; i < pos.count; i++) {
    const t = THREE.MathUtils.clamp(pos.getY(i) / 520, -1, 1);
    if (t >= 0) tmp.copy(midCol).lerp(topCol, Math.pow(t, 0.65));
    else tmp.copy(midCol).lerp(botCol, Math.min(1, -t * 2));
    cols[i * 3] = tmp.r;
    cols[i * 3 + 1] = tmp.g;
    cols[i * 3 + 2] = tmp.b;
  }
  skyGeo.setAttribute('color', new THREE.BufferAttribute(cols, 3));
  scene.add(new THREE.Mesh(skyGeo, new THREE.MeshBasicMaterial({ vertexColors: true, side: THREE.BackSide, fog: false })));

  // Extended surrounding ocean plane just under the diorama water level
  const seaPlane = new THREE.Mesh(
    new THREE.PlaneGeometry(1600, 1600),
    new THREE.MeshStandardMaterial({ color: 0x2c6482, roughness: 0.18, metalness: 0.08 })
  );
  seaPlane.rotation.x = -Math.PI / 2;
  seaPlane.position.y = SEA_Y - 0.12;
  scene.add(seaPlane);
}

// ============================================================================
// Exact Spatial-Grid Terrain Triangle Query (zero floating/sinking!)
// ============================================================================
class TerrainSurface {
  constructor(gridN = 32) {
    this.gridN = gridN;
    this.buckets = Array.from({ length: gridN * gridN }, () => []);
    this.splatData = null;
    this.splatSize = 256;
    this.heightData = null;
    this.heightSize = 256;
  }

  buildFromMesh(root) {
    root.updateWorldMatrix(true, true);
    const a = new THREE.Vector3(), b = new THREE.Vector3(), c = new THREE.Vector3();
    const ab = new THREE.Vector3(), ac = new THREE.Vector3(), n = new THREE.Vector3();

    root.traverse((obj) => {
      if (!obj.isMesh) return;
      // Skip translucent water mesh
      const mat = obj.material;
      if (mat && mat.transparent) return;

      const geo = obj.geometry;
      const pos = geo.attributes.position;
      const idx = geo.index;
      const count = idx ? idx.count : pos.count;
      for (let i = 0; i < count; i += 3) {
        const i0 = idx ? idx.getX(i) : i;
        const i1 = idx ? idx.getX(i + 1) : i + 1;
        const i2 = idx ? idx.getX(i + 2) : i + 2;
        a.fromBufferAttribute(pos, i0).applyMatrix4(obj.matrixWorld);
        b.fromBufferAttribute(pos, i1).applyMatrix4(obj.matrixWorld);
        c.fromBufferAttribute(pos, i2).applyMatrix4(obj.matrixWorld);
        ab.subVectors(b, a);
        ac.subVectors(c, a);
        n.crossVectors(ab, ac);
        const len = n.length();
        if (len < 1e-6) continue;
        n.divideScalar(len);
        // Only upward-facing terrain surface triangles (ignore vertical skirt & bottom cap)
        if (n.y < 0.25) continue;

        const tri = {
          ax: a.x, ay: a.y, az: a.z,
          bx: b.x, by: b.y, bz: b.z,
          cx: c.x, cy: c.y, cz: c.z,
          nx: n.x, ny: n.y, nz: n.z,
        };
        const minX = Math.min(a.x, b.x, c.x), maxX = Math.max(a.x, b.x, c.x);
        const minZ = Math.min(a.z, b.z, c.z), maxZ = Math.max(a.z, b.z, c.z);
        const gx0 = this.toCell(minX), gx1 = this.toCell(maxX);
        const gz0 = this.toCell(minZ), gz1 = this.toCell(maxZ);
        for (let gz = gz0; gz <= gz1; gz++) {
          for (let gx = gx0; gx <= gx1; gx++) {
            this.buckets[gz * this.gridN + gx].push(tri);
          }
        }
      }
    });
  }

  toCell(coord) {
    const u = (coord / WORLD_W + 0.5) * this.gridN;
    return Math.max(0, Math.min(this.gridN - 1, Math.floor(u)));
  }

  sample(x, z, outNormal = null) {
    const gx = this.toCell(x);
    const gz = this.toCell(z);
    const tris = this.buckets[gz * this.gridN + gx];
    let bestY = -Infinity;
    let bestTri = null;

    for (let i = 0; i < tris.length; i++) {
      const t = tris[i];
      // 2D barycentric test in XZ plane
      const v0x = t.cx - t.ax, v0z = t.cz - t.az;
      const v1x = t.bx - t.ax, v1z = t.bz - t.az;
      const v2x = x - t.ax, v2z = z - t.az;
      const dot00 = v0x * v0x + v0z * v0z;
      const dot01 = v0x * v1x + v0z * v1z;
      const dot02 = v0x * v2x + v0z * v2z;
      const dot11 = v1x * v1x + v1z * v1z;
      const dot12 = v1x * v2x + v1z * v2z;
      const denom = dot00 * dot11 - dot01 * dot01;
      if (Math.abs(denom) < 1e-8) continue;
      const inv = 1 / denom;
      const u = (dot11 * dot02 - dot01 * dot12) * inv;
      const v = (dot00 * dot12 - dot01 * dot02) * inv;
      if (u >= -0.002 && v >= -0.002 && u + v <= 1.002) {
        const y = t.ay + u * (t.cy - t.ay) + v * (t.by - t.ay);
        if (y > bestY) {
          bestY = y;
          bestTri = t;
        }
      }
    }

    if (bestTri) {
      if (outNormal) outNormal.set(bestTri.nx, bestTri.ny, bestTri.nz);
      return bestY;
    }
    if (outNormal) outNormal.set(0, 1, 0);
    return TERRAIN_BASE_Y;
  }

  sampleBiome(y, slopeDeg) {
    const h = THREE.MathUtils.clamp((y - TERRAIN_BASE_Y) / WORLD_H, 0, 1);
    const sand = (h < 0.19 && slopeDeg < 26) ? 1 : 0;
    const snow = (h > 0.68 && slopeDeg < 58) ? 1 : 0;
    const rock = (slopeDeg >= 27) ? 1 : 0;
    const grass = (!sand && !snow && !rock) ? 1 : 0;
    return [sand, grass, rock, snow];
  }
}

const terrainSurface = new TerrainSurface(32);

// ============================================================================
// Web Audio Engine (100% Artisan-compiled WAV loops & one-shots)
// ============================================================================
class SoundEngine {
  constructor() {
    this.ctx = null;
    this.buffers = {};
    this.masterGain = null;
    this.musicGain = null;
    this.ambGain = null;
    this.sfxGain = null;
    this.engineSrc = null;
    this.engineGain = null;
    this.started = false;
    this.muted = false;
    this.volumes = { music: 0.35, amb: 0.65, sfx: 0.75 };
  }

  async preload(urls) {
    const entries = await Promise.all(
      Object.entries(urls).map(async ([k, url]) => {
        const res = await fetch(url);
        const ab = await res.arrayBuffer();
        return [k, ab];
      })
    );
    this.rawBytes = Object.fromEntries(entries);
  }

  async start() {
    if (this.started) {
      if (this.ctx && this.ctx.state === 'suspended') await this.ctx.resume();
      return;
    }
    const AudioCtx = window.AudioContext || window.webkitAudioContext;
    this.ctx = new AudioCtx();
    this.masterGain = this.ctx.createGain();
    this.musicGain = this.ctx.createGain();
    this.ambGain = this.ctx.createGain();
    this.sfxGain = this.ctx.createGain();
    this.engineGain = this.ctx.createGain();

    this.musicGain.connect(this.masterGain);
    this.ambGain.connect(this.masterGain);
    this.sfxGain.connect(this.masterGain);
    this.engineGain.connect(this.sfxGain);
    this.masterGain.connect(this.ctx.destination);
    this.applyVolumes();

    for (const [k, ab] of Object.entries(this.rawBytes || {})) {
      this.buffers[k] = await this.ctx.decodeAudioData(ab.slice(0));
    }

    // Start seamless loops
    if (this.buffers.ambience) {
      const s = this.ctx.createBufferSource();
      s.buffer = this.buffers.ambience;
      s.loop = true;
      s.connect(this.ambGain);
      s.start();
    }
    if (this.buffers.music) {
      const s = this.ctx.createBufferSource();
      s.buffer = this.buffers.music;
      s.loop = true;
      s.connect(this.musicGain);
      s.start();
    }
    if (this.buffers.engine) {
      const s = this.ctx.createBufferSource();
      s.buffer = this.buffers.engine;
      s.loop = true;
      s.playbackRate.value = 0.9;
      s.connect(this.engineGain);
      s.start();
      this.engineSrc = s;
      this.engineGain.gain.value = 0.22;
    }
    this.started = true;
  }

  applyVolumes() {
    if (!this.ctx) return;
    const now = this.ctx.currentTime;
    this.masterGain.gain.setTargetAtTime(this.muted ? 0 : 1, now, 0.04);
    this.musicGain.gain.setTargetAtTime(this.volumes.music * 0.28, now, 0.05);
    this.ambGain.gain.setTargetAtTime(this.volumes.amb * 0.42, now, 0.05);
    this.sfxGain.gain.setTargetAtTime(this.volumes.sfx * 0.85, now, 0.05);
  }

  updateEngine(speedRatio, throttleActive, paused) {
    if (!this.started || !this.engineSrc) return;
    const now = this.ctx.currentTime;
    const targetRate = paused ? 0.82 : 0.86 + speedRatio * 1.45 + (throttleActive ? 0.22 : 0);
    const targetGain = paused ? 0.03 : 0.16 + speedRatio * 0.22 + (throttleActive ? 0.12 : 0);
    this.engineSrc.playbackRate.setTargetAtTime(targetRate, now, 0.08);
    this.engineGain.gain.setTargetAtTime(targetGain, now, 0.08);
  }

  playOneShot(name, pitch = 1.0, gain = 0.85) {
    if (!this.started || !this.buffers[name]) return;
    const src = this.ctx.createBufferSource();
    const g = this.ctx.createGain();
    src.buffer = this.buffers[name];
    src.playbackRate.value = pitch;
    g.gain.value = gain;
    src.connect(g);
    g.connect(this.sfxGain);
    src.start();
  }
}

const sound = new SoundEngine();

// ============================================================================
// Game State & World Objects
// ============================================================================
let mode = 'loading'; // 'loading' | 'start' | 'playing' | 'paused' | 'victory'
let cameraMode = 0;   // 0: Chase, 1: Hood, 2: Orbit
const CAM_LABELS = ['CHASE', 'HOOD', 'ORBIT'];

const keys = {};
const obstacles = []; // { x, z, r }
const coins = [];     // { mesh, x, y, z, collected, baseY, phase }
let collectedCount = 0;
let runTime = 0;
let streakCount = 0;
let lastCoinTime = -10;
let foxDiscovered = false;
let toastTimer = 0;
let headlight = null;
let headlightFlashTimer = 0;
let lastHornTime = -10;
let foxModel = null;
let foxBaseY = 0;
let foxHopTimer = 0;

// Off-road 4x4 vehicle state
const vehicleRoot = new THREE.Group();
const vehicleBody = new THREE.Group();
vehicleRoot.add(vehicleBody);
scene.add(vehicleRoot);

const wheelPivots = []; // { pivot, spinMesh, lx, lz, isFront }
const car = {
  x: CAR_START.x,
  y: 25,
  z: CAR_START.z,
  vy: 0,
  yaw: CAR_START.yaw,
  speed: 0,
  steer: 0,
  wheelSpin: 0,
  grounded: true,
  pitch: 0,
  roll: 0,
  bodyRoll: 0,
};

// Minimap background buffer
const minimapBg = document.createElement('canvas');
minimapBg.width = 148;
minimapBg.height = 148;

// Dust / splash particle pool
const PARTICLE_COUNT = 72;
const particles = [];
const particleGeo = new THREE.DodecahedronGeometry(0.24, 0);
const dustMat = new THREE.MeshLambertMaterial({ color: 0xd9c69e, transparent: true, opacity: 0.65 });
const splashMat = new THREE.MeshLambertMaterial({ color: 0x8ad8f0, transparent: true, opacity: 0.72 });
let particleCursor = 0;

function initParticles() {
  for (let i = 0; i < PARTICLE_COUNT; i++) {
    const m = new THREE.Mesh(particleGeo, dustMat);
    m.visible = false;
    scene.add(m);
    particles.push({ mesh: m, vx: 0, vy: 0, vz: 0, life: 0, maxLife: 0.55 });
  }
}

function spawnParticle(x, y, z, vx, vy, vz, isWater) {
  const p = particles[particleCursor];
  particleCursor = (particleCursor + 1) % PARTICLE_COUNT;
  p.mesh.position.set(x, y, z);
  p.mesh.material = isWater ? splashMat : dustMat;
  p.mesh.visible = true;
  p.mesh.scale.setScalar(0.6 + Math.random() * 0.7);
  p.vx = vx;
  p.vy = vy;
  p.vz = vz;
  p.life = 0;
  p.maxLife = isWater ? 0.42 : 0.62;
}

// ============================================================================
// Asset Loading & World Population
// ============================================================================
function loadImagePixels(url) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.crossOrigin = 'anonymous';
    img.onload = () => {
      const c = document.createElement('canvas');
      c.width = img.width;
      c.height = img.height;
      const ctx = c.getContext('2d');
      ctx.drawImage(img, 0, 0);
      resolve({ width: img.width, height: img.height, data: ctx.getImageData(0, 0, img.width, img.height).data });
    };
    img.onerror = reject;
    img.src = url;
  });
}

function buildMinimapBase(heightImg) {
  const ctx = minimapBg.getContext('2d');
  const w = minimapBg.width, h = minimapBg.height;
  const out = ctx.createImageData(w, h);
  const hw = heightImg.width, hh = heightImg.height;
  const cellWorld = WORLD_W / hw;

  for (let y = 0; y < h; y++) {
    const sy = Math.min(hh - 1, Math.floor((y / h) * hh));
    const sym = Math.max(0, sy - 1);
    const syp = Math.min(hh - 1, sy + 1);
    for (let x = 0; x < w; x++) {
      const sx = Math.min(hw - 1, Math.floor((x / w) * hw));
      const sxm = Math.max(0, sx - 1);
      const sxp = Math.min(hw - 1, sx + 1);

      const hv = heightImg.data[(sy * hw + sx) * 4] / 255;
      const hL = heightImg.data[(sy * hw + sxm) * 4] / 255;
      const hR = heightImg.data[(sy * hw + sxp) * 4] / 255;
      const hU = heightImg.data[(sym * hw + sx) * 4] / 255;
      const hD = heightImg.data[(syp * hw + sx) * 4] / 255;

      const gx = ((hR - hL) * WORLD_H) / (2 * cellWorld);
      const gz = ((hD - hU) * WORLD_H) / (2 * cellWorld);
      const slopeDeg = Math.atan(Math.hypot(gx, gz)) * (180 / Math.PI);
      const shade = THREE.MathUtils.clamp(0.86 + ((hL - hR) + (hU - hD)) * 5.5, 0.58, 1.22);

      const o = (y * w + x) * 4;
      if (hv <= SEA_LEVEL_NORM) {
        const depth = THREE.MathUtils.clamp((SEA_LEVEL_NORM - hv) / SEA_LEVEL_NORM, 0, 1);
        out.data[o] = 44 - depth * 16;
        out.data[o + 1] = 102 - depth * 28;
        out.data[o + 2] = 136 - depth * 24;
        out.data[o + 3] = 255;
      } else {
        let col;
        if (hv > 0.70 && slopeDeg < 58) col = [234, 232, 222];      // alpine snow
        else if (slopeDeg >= 26) col = [118, 126, 138];              // cliff rock
        else if (hv < 0.175) col = [212, 191, 148];                  // coastal sand
        else col = [86, 136, 62];                                    // highland grass
        out.data[o] = Math.min(255, col[0] * shade);
        out.data[o + 1] = Math.min(255, col[1] * shade);
        out.data[o + 2] = Math.min(255, col[2] * shade);
        out.data[o + 3] = 255;
      }
    }
  }
  ctx.putImageData(out, 0, 0);
}

const COLLIDER_NODE_RE = /(-colonly|-convcolonly|-col|-convcol)$|^(UCX_|UBX_|USP_|UCP_)/i;

function stripColliderNodes(root) {
  const toRemove = [];
  root.traverse((o) => {
    if (o.name && COLLIDER_NODE_RE.test(o.name)) {
      toRemove.push(o);
    }
  });
  for (const node of toRemove) {
    if (node.parent) node.parent.remove(node);
  }
}

function enableShadows(root, cast = true, receive = true) {
  root.traverse((o) => {
    if (o.isMesh) {
      o.castShadow = cast && !(o.material && o.material.transparent);
      o.receiveShadow = receive;
    }
  });
}

function extractFirstMesh(gltfScene) {
  let found = null;
  gltfScene.traverse((o) => {
    if (!found && o.isMesh) found = o;
  });
  return found;
}

function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a |= 0; a = (a + 0x6D2B79F5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function populateInstancedProp(templateMesh, count, predicate, seed, minDist, scaleRange, addColliderRadius = 0) {
  const rng = mulberry32(seed);
  const matrices = [];
  const dummy = new THREE.Object3D();
  const nrm = new THREE.Vector3();
  const placed = [];

  let attempts = 0;
  while (matrices.length < count && attempts < count * 28) {
    attempts++;
    const x = (rng() - 0.5) * (WORLD_W - 24);
    const z = (rng() - 0.5) * (WORLD_D - 24);

    // Keep cabin clearing, spawn yard, and fox overlook tidy
    if (Math.hypot(x - CABIN_POS.x, z - CABIN_POS.y) < 20) continue;
    if (Math.hypot(x - CAR_START.x, z - CAR_START.z) < 12) continue;
    if (Math.hypot(x - FOX_POS.x, z - FOX_POS.y) < 10) continue;

    // Keep clear of coin spots
    let nearCoin = false;
    for (const [cx, cz] of COIN_COORDS) {
      if (Math.hypot(x - cx, z - cz) < 6.5) { nearCoin = true; break; }
    }
    if (nearCoin) continue;

    const y = terrainSurface.sample(x, z, nrm);
    if (y < SEA_Y + 0.55) continue;
    const slopeDeg = Math.acos(THREE.MathUtils.clamp(nrm.y, 0, 1)) * 180 / Math.PI;
    const splat = terrainSurface.sampleBiome(y, slopeDeg);
    if (!predicate(y, slopeDeg, splat, rng)) continue;

    let tooClose = false;
    for (let i = 0; i < placed.length; i++) {
      if (Math.hypot(x - placed[i][0], z - placed[i][1]) < minDist) { tooClose = true; break; }
    }
    if (tooClose) continue;

    const sc = scaleRange[0] + rng() * (scaleRange[1] - scaleRange[0]);
    dummy.position.set(x, y - 0.12, z);
    dummy.rotation.set((rng() - 0.5) * 0.08, rng() * Math.PI * 2, (rng() - 0.5) * 0.08);
    dummy.scale.setScalar(sc);
    dummy.updateMatrix();
    matrices.push(dummy.matrix.clone());
    placed.push([x, z]);

    if (addColliderRadius > 0) {
      obstacles.push({ x, z, r: addColliderRadius * sc });
    }
  }

  const inst = new THREE.InstancedMesh(templateMesh.geometry, templateMesh.material, matrices.length);
  inst.castShadow = true;
  inst.receiveShadow = true;
  for (let i = 0; i < matrices.length; i++) {
    inst.setMatrixAt(i, matrices[i]);
  }
  inst.instanceMatrix.needsUpdate = true;
  scene.add(inst);
}

function populateMeadowPatches(templateMesh, patchCount, perPatch, spread, seed, scaleRange, bonusSpots = []) {
  const rng = mulberry32(seed);
  const matrices = [];
  const dummy = new THREE.Object3D();
  const nrm = new THREE.Vector3();
  const up = new THREE.Vector3(0, 1, 0);
  const tiltQ = new THREE.Quaternion();
  const yawQ = new THREE.Quaternion();

  const tryAddCluster = (cx, cz, count) => {
    for (let j = 0; j < count; j++) {
      const ang = rng() * Math.PI * 2;
      const rad = Math.sqrt(rng()) * spread;
      const x = cx + Math.cos(ang) * rad;
      const z = cz + Math.sin(ang) * rad;
      if (Math.abs(x) > WORLD_W * 0.47 || Math.abs(z) > WORLD_D * 0.47) continue;
      // Keep cabin footprint, immediate spawn hood & fox stone clear
      if (Math.hypot(x - CABIN_POS.x, z - CABIN_POS.y) < 8.5) continue;
      if (Math.hypot(x - FOX_POS.x, z - FOX_POS.y) < 3.4) continue;

      const y = terrainSurface.sample(x, z, nrm);
      if (y < SEA_Y + 0.9 || y > TERRAIN_BASE_Y + WORLD_H * 0.62) continue;
      const slopeDeg = Math.acos(THREE.MathUtils.clamp(nrm.y, 0, 1)) * 180 / Math.PI;
      const splat = terrainSurface.sampleBiome(y, slopeDeg);
      if (splat[1] < 0.35 || slopeDeg > 24) continue;

      const sc = scaleRange[0] + rng() * (scaleRange[1] - scaleRange[0]);
      dummy.position.set(x, y - 0.04, z);
      // Gently align to terrain slope (45% blend) + random heading
      const blendN = up.clone().lerp(nrm, 0.45).normalize();
      tiltQ.setFromUnitVectors(up, blendN);
      yawQ.setFromAxisAngle(up, rng() * Math.PI * 2);
      dummy.quaternion.copy(tiltQ).multiply(yawQ);
      dummy.scale.setScalar(sc);
      dummy.updateMatrix();
      matrices.push(dummy.matrix.clone());
    }
  };

  for (const [bx, bz] of bonusSpots) {
    tryAddCluster(bx, bz, perPatch);
  }

  let placedPatches = 0;
  let attempts = 0;
  while (placedPatches < patchCount && attempts < patchCount * 16) {
    attempts++;
    const cx = (rng() - 0.5) * (WORLD_W - 28);
    const cz = (rng() - 0.5) * (WORLD_D - 28);
    const y = terrainSurface.sample(cx, cz, nrm);
    if (y < SEA_Y + 1.0 || y > TERRAIN_BASE_Y + WORLD_H * 0.60) continue;
    const slopeDeg = Math.acos(THREE.MathUtils.clamp(nrm.y, 0, 1)) * 180 / Math.PI;
    const splat = terrainSurface.sampleBiome(y, slopeDeg);
    if (splat[1] < 0.40 || slopeDeg > 22) continue;

    const before = matrices.length;
    tryAddCluster(cx, cz, Math.max(2, Math.round(perPatch * (0.7 + rng() * 0.6))));
    if (matrices.length > before) placedPatches++;
  }

  const inst = new THREE.InstancedMesh(templateMesh.geometry, templateMesh.material, matrices.length);
  inst.castShadow = false;
  inst.receiveShadow = true;
  for (let i = 0; i < matrices.length; i++) {
    inst.setMatrixAt(i, matrices[i]);
  }
  inst.instanceMatrix.needsUpdate = true;
  scene.add(inst);
}

async function loadAllAssets() {
  const loader = new GLTFLoader();
  const loadGltf = (url) =>
    new Promise((res, rej) =>
      loader.load(
        url,
        (gltf) => {
          stripColliderNodes(gltf.scene);
          res(gltf);
        },
        undefined,
        rej
      )
    );

  let loaded = 0;
  const totalSteps = 12;
  const step = (msg) => {
    loaded++;
    loadingText.textContent = msg;
    loadingBar.style.width = `${Math.round((loaded / totalSteps) * 100)}%`;
  };

  step('Loading 400×400m eroded terrain…');
  const [terrainGltf, heightImg] = await Promise.all([
    loadGltf('assets/mountain_trail.glb'),
    loadImagePixels('assets/mountain_trail_height16.png'),
  ]);
  enableShadows(terrainGltf.scene, false, true);
  scene.add(terrainGltf.scene);
  terrainSurface.heightData = heightImg.data;
  terrainSurface.heightSize = heightImg.width;
  terrainSurface.buildFromMesh(terrainGltf.scene);
  buildMinimapBase(heightImg);
  step('Building 4×4 vehicle & wheels…');

  const [chassisGltf, wheelGltf] = await Promise.all([
    loadGltf('assets/offroad_chassis.glb'),
    loadGltf('assets/offroad_wheel.glb'),
  ]);
  enableShadows(chassisGltf.scene, true, true);
  vehicleBody.add(chassisGltf.scene);

  // Mount 4 independent wheels at exact chassis axle coordinates
  const wheelCoords = [
    { lx: 0.84, lz: 1.08, isFront: true },
    { lx: -0.84, lz: 1.08, isFront: true },
    { lx: 0.84, lz: -1.04, isFront: false },
    { lx: -0.84, lz: -1.04, isFront: false },
  ];
  for (const wc of wheelCoords) {
    const pivot = new THREE.Group();
    pivot.position.set(wc.lx, 0.36, wc.lz);
    const spinMesh = wheelGltf.scene.clone(true);
    enableShadows(spinMesh, true, true);
    pivot.add(spinMesh);
    vehicleRoot.add(pivot);
    wheelPivots.push({ pivot, spinMesh, ...wc });
  }

  // Subtle warm front headlight cone pool (flashes brightly on horn!)
  headlight = new THREE.SpotLight(0xfff0b8, 22, 34, Math.PI * 0.28, 0.6, 1.4);
  headlight.position.set(0, 1.1, 1.8);
  headlight.target.position.set(0, 0.2, 16);
  vehicleBody.add(headlight);
  vehicleBody.add(headlight.target);

  step('Placing mountain cabin & red fox landmark…');
  const [cabinGltf, foxGltf] = await Promise.all([
    loadGltf('assets/mountain_cabin.glb'),
    loadGltf('assets/fox_on_stones.glb'),
  ]);
  enableShadows(cabinGltf.scene, true, true);
  // Sample cabin center & front porch steps; deep 2.88m stone foundation anchors solidly on any slope
  const cabinCenterY = terrainSurface.sample(CABIN_POS.x, CABIN_POS.y);
  const cabinPorchY = terrainSurface.sample(CABIN_POS.x, CABIN_POS.y + 4.3);
  const cabinY = Math.max(cabinCenterY, cabinPorchY) - 0.14;
  cabinGltf.scene.position.set(CABIN_POS.x, cabinY, CABIN_POS.y);
  scene.add(cabinGltf.scene);
  obstacles.push({ x: CABIN_POS.x, z: CABIN_POS.y, r: 5.2 });

  // Warm cozy porch lantern glow
  const porchLight = new THREE.PointLight(0xffb84d, 14, 18, 1.6);
  porchLight.position.set(CABIN_POS.x + 0.9, cabinY + 2.8, CABIN_POS.y + 3.0);
  scene.add(porchLight);

  // Red Fox on Carved Stones perched on the northeast overlook (grounded cleanly on slope)
  enableShadows(foxGltf.scene, true, true);
  const foxC = terrainSurface.sample(FOX_POS.x, FOX_POS.y);
  const foxMinCorner = Math.min(
    foxC,
    terrainSurface.sample(FOX_POS.x + 1.1, FOX_POS.y + 1.0),
    terrainSurface.sample(FOX_POS.x - 1.1, FOX_POS.y + 1.0),
    terrainSurface.sample(FOX_POS.x + 1.1, FOX_POS.y - 1.0),
    terrainSurface.sample(FOX_POS.x - 1.1, FOX_POS.y - 1.0)
  );
  foxBaseY = Math.max(foxC - 0.34, foxMinCorner - 0.04);
  foxGltf.scene.position.set(FOX_POS.x, foxBaseY, FOX_POS.y);
  foxGltf.scene.scale.setScalar(1.75);
  foxGltf.scene.rotation.y = -0.65;
  foxModel = foxGltf.scene;
  scene.add(foxGltf.scene);
  obstacles.push({ x: FOX_POS.x, z: FOX_POS.y, r: 1.8 });

  step('Planting instanced alpine forest, grass & wildflowers…');
  const [pineGltf, rockGltf, bushGltf, grassGltf, flowerGltf, coinGltf] = await Promise.all([
    loadGltf('assets/pine_tree.glb'),
    loadGltf('assets/boulder_rock.glb'),
    loadGltf('assets/bush_foliage.glb'),
    loadGltf('assets/grass_tuft.glb'),
    loadGltf('assets/wildflowers.glb'),
    loadGltf('assets/trail_coin.glb'),
  ]);

  const pineMesh = extractFirstMesh(pineGltf.scene);
  const rockMesh = extractFirstMesh(rockGltf.scene);
  const bushMesh = extractFirstMesh(bushGltf.scene);
  const grassMesh = extractFirstMesh(grassGltf.scene);
  const flowerMesh = extractFirstMesh(flowerGltf.scene);

  // 260 Alpine pines on grassy slopes
  populateInstancedProp(
    pineMesh,
    260,
    (y, slope, splat) => splat[1] > 0.35 && slope < 25 && y < TERRAIN_BASE_Y + WORLD_H * 0.68,
    101,
    5.5,
    [0.85, 1.55],
    0.55
  );
  // 180 Foliage bushes on meadows
  populateInstancedProp(
    bushMesh,
    180,
    (y, slope, splat) => splat[1] > 0.30 && slope < 26 && y < TERRAIN_BASE_Y + WORLD_H * 0.60,
    202,
    4.0,
    [0.75, 1.45],
    0
  );
  // 85 Mossy alpine boulders on foothills & rocky ridges
  populateInstancedProp(
    rockMesh,
    85,
    (y, slope, splat) => (splat[2] > 0.22 || splat[0] > 0.35 || slope > 16) && slope < 36,
    303,
    7.5,
    [0.7, 1.85],
    1.1
  );
  // Patchy meadow grass tufts (~340 total) & colorful alpine wildflowers (~170 total) — zero collision
  const scenicMeadowSpots = [
    [CABIN_POS.x - 12, CABIN_POS.y + 8],
    [CABIN_POS.x + 12, CABIN_POS.y + 6],
    [CABIN_POS.x + 8, CABIN_POS.y - 10],
    [FOX_POS.x - 6, FOX_POS.y + 5],
    [FOX_POS.x + 6, FOX_POS.y - 4],
  ];
  populateMeadowPatches(grassMesh, 72, 4, 7.0, 404, [0.75, 1.35], scenicMeadowSpots);
  populateMeadowPatches(flowerMesh, 46, 3, 5.5, 505, [0.78, 1.18], scenicMeadowSpots);

  step('Placing 18 gold expedition coins…');
  // Subtle tall glowing beacon ring under each coin for easy long-range sightlines
  const beaconGeo = new THREE.CylinderGeometry(0.18, 0.55, 6.5, 10, 1, true);
  const beaconMat = new THREE.MeshBasicMaterial({
    color: 0xffd55a,
    transparent: true,
    opacity: 0.14,
    side: THREE.DoubleSide,
    depthWrite: false,
  });

  COIN_COORDS.forEach(([cx, cz], idx) => {
    const cy = Math.max(SEA_Y + 0.2, terrainSurface.sample(cx, cz));
    const group = new THREE.Group();
    const cModel = coinGltf.scene.clone(true);
    enableShadows(cModel, true, false);
    cModel.scale.setScalar(1.15);
    group.add(cModel);

    const beam = new THREE.Mesh(beaconGeo, beaconMat);
    beam.position.y = 3.5;
    group.add(beam);

    group.position.set(cx, cy + 0.35, cz);
    scene.add(group);
    coins.push({
      mesh: group,
      coinModel: cModel,
      x: cx,
      y: cy + 0.35,
      z: cz,
      baseY: cy + 0.35,
      collected: false,
      phase: idx * 0.6,
    });
  });

  initParticles();

  step('Preloading Artisan procedural audio…');
  await sound.preload({
    ambience: 'assets/woodland_ambience.wav',
    music: 'assets/arcade_anthem_loop.wav',
    engine: 'assets/engine_loop.wav',
    coin: 'assets/coin_pickup.wav',
    bump: 'assets/bump_thud.wav',
    horn: 'assets/car_horn.wav',
  });

  resetVehicle();
  loadingBar.style.width = '100%';
  setTimeout(() => {
    loadingScreen.classList.add('hidden');
    startMenu.classList.remove('hidden');
    mode = 'start';
  }, 200);
}

// ============================================================================
// Vehicle Physics & Controls
// ============================================================================
function resetVehicle() {
  car.x = CAR_START.x;
  car.z = CAR_START.z;
  car.y = terrainSurface.sample(car.x, car.z) + 0.1;
  car.vy = 0;
  car.yaw = CAR_START.yaw;
  car.speed = 0;
  car.steer = 0;
  car.pitch = 0;
  car.roll = 0;
  car.bodyRoll = 0;
  car.grounded = true;
  syncVehicleTransform();
}

function resetRun() {
  collectedCount = 0;
  runTime = 0;
  streakCount = 0;
  foxDiscovered = false;
  for (const c of coins) {
    c.collected = false;
    c.mesh.visible = true;
  }
  hudCoins.textContent = `0 / ${coins.length}`;
  resetVehicle();
}

function showToast(msg, duration = 2.4) {
  toastEl.textContent = msg;
  toastEl.classList.remove('hidden');
  toastTimer = duration;
}

const tmpNrm = new THREE.Vector3();
function updateVehicle(dt) {
  const fwdInput = (keys['KeyW'] || keys['ArrowUp'] ? 1 : 0) - (keys['KeyS'] || keys['ArrowDown'] ? 1 : 0);
  const steerInput = (keys['KeyA'] || keys['ArrowLeft'] ? 1 : 0) - (keys['KeyD'] || keys['ArrowRight'] ? 1 : 0);
  const boost = Boolean(keys['ShiftLeft'] || keys['ShiftRight']);
  const brake = Boolean(keys['Space']);

  // Steering response (slightly tamer at high speed for smooth trail control)
  const speedRatio = Math.min(1, Math.abs(car.speed) / 24);
  const maxSteer = THREE.MathUtils.lerp(0.62, 0.36, speedRatio);
  car.steer = THREE.MathUtils.lerp(car.steer, steerInput * maxSteer, Math.min(1, dt * 9));

  const forwardX = Math.sin(car.yaw);
  const forwardZ = Math.cos(car.yaw);

  // Sample terrain normal under vehicle center for realistic hill climb gravity
  const groundC = terrainSurface.sample(car.x, car.z, tmpNrm);
  const slopeAlongFwd = -(tmpNrm.x * forwardX + tmpNrm.z * forwardZ); // negative when climbing uphill

  // Shallow water wading drag
  const inWater = car.y < SEA_Y - 0.05;
  const maxFwdSpeed = (boost ? 28.0 : 21.0) * (inWater ? 0.62 : 1.0);
  const accel = (boost ? 26.0 : 18.5) * (inWater ? 0.72 : 1.0);

  if (car.grounded) {
    if (fwdInput > 0) {
      car.speed += accel * dt;
    } else if (fwdInput < 0) {
      car.speed -= (car.speed > 1 ? 26.0 : 13.0) * dt;
    } else {
      // Natural rolling friction
      car.speed *= Math.exp(-1.6 * dt);
    }

    if (brake) {
      car.speed *= Math.exp(-5.5 * dt);
    }

    // Hill gravity pull (gentle enough to climb alpine ridges with Shift boost!)
    car.speed += slopeAlongFwd * 13.5 * dt;
    car.speed = THREE.MathUtils.clamp(car.speed, -10, maxFwdSpeed);

    // Turning rate proportional to speed (smooth low-speed turn-in)
    if (Math.abs(car.speed) > 0.12) {
      const turnDir = car.speed >= 0 ? 1 : -1;
      const turnFactor = Math.min(1, Math.abs(car.speed) / 4.5);
      car.yaw += car.steer * 2.15 * turnFactor * turnDir * dt;
    }
  }

  // Integrate horizontal position
  const nextX = car.x + Math.sin(car.yaw) * car.speed * dt;
  const nextZ = car.z + Math.cos(car.yaw) * car.speed * dt;

  // Keep vehicle inside island playable bounds
  const bound = WORLD_W * 0.5 - 8;
  car.x = THREE.MathUtils.clamp(nextX, -bound, bound);
  car.z = THREE.MathUtils.clamp(nextZ, -bound, bound);

  // Push gently back from deep ocean water
  if (groundC < SEA_Y - 1.35) {
    const toCenter = Math.atan2(-car.x, -car.z);
    car.x += Math.sin(toCenter) * 4 * dt;
    car.z += Math.cos(toCenter) * 4 * dt;
    car.speed *= 0.92;
  }

  // Soft circle collision against cabin, fox stones, tree trunks & boulders
  const carR = 1.25;
  for (let i = 0; i < obstacles.length; i++) {
    const ob = obstacles[i];
    const dx = car.x - ob.x;
    const dz = car.z - ob.z;
    const minD = carR + ob.r;
    const d2 = dx * dx + dz * dz;
    if (d2 < minD * minD && d2 > 1e-6) {
      const d = Math.sqrt(d2);
      const nx = dx / d, nz = dz / d;
      car.x = ob.x + nx * minD;
      car.z = ob.z + nz * minD;
      if (Math.abs(car.speed) > 4.5) {
        sound.playOneShot('bump', 0.9 + Math.random() * 0.2, Math.min(0.8, Math.abs(car.speed) / 18));
      }
      car.speed *= -0.25;
    }
  }

  // 4-wheel exact terrain contact for realistic suspension pitch, roll & airtime
  const sinY = Math.sin(car.yaw), cosY = Math.cos(car.yaw);
  const wPos = (lx, lz) => [car.x + lx * cosY + lz * sinY, car.z - lx * sinY + lz * cosY];
  const [flx, flz] = wPos(0.84, 1.08);
  const [frx, frz] = wPos(-0.84, 1.08);
  const [rlx, rlz] = wPos(0.84, -1.04);
  const [rrx, rrz] = wPos(-0.84, -1.04);

  const yFL = terrainSurface.sample(flx, flz);
  const yFR = terrainSurface.sample(frx, frz);
  const yRL = terrainSurface.sample(rlx, rlz);
  const yRR = terrainSurface.sample(rrx, rrz);
  const targetGroundY = (yFL + yFR + yRL + yRR) * 0.25;

  const wasGrounded = car.grounded;
  car.vy -= 22.0 * dt;
  car.y += car.vy * dt;

  if (car.y <= targetGroundY) {
    if (!wasGrounded && car.vy < -4.2) {
      sound.playOneShot('bump', 0.85 + Math.random() * 0.2, Math.min(0.95, Math.abs(car.vy) / 14));
    }
    car.y = targetGroundY;
    car.vy = 0;
    car.grounded = true;
  } else if (car.y - targetGroundY < 0.22 && car.vy <= 0) {
    car.y = THREE.MathUtils.lerp(car.y, targetGroundY, Math.min(1, dt * 18));
    car.vy = 0;
    car.grounded = true;
  } else {
    car.grounded = false;
  }

  // Compute chassis pitch & roll from the 4 wheel heights
  const frontAvg = (yFL + yFR) * 0.5;
  const rearAvg = (yRL + yRR) * 0.5;
  const leftAvg = (yFL + yRL) * 0.5;
  const rightAvg = (yFR + yRR) * 0.5;
  const targetPitch = -Math.atan2(frontAvg - rearAvg, 2.12);
  const targetRoll = Math.atan2(leftAvg - rightAvg, 1.68);
  car.pitch = THREE.MathUtils.lerp(car.pitch, targetPitch, Math.min(1, dt * 10));
  car.roll = THREE.MathUtils.lerp(car.roll, targetRoll, Math.min(1, dt * 10));
  car.bodyRoll = THREE.MathUtils.lerp(car.bodyRoll, -car.steer * speedRatio * 0.14, Math.min(1, dt * 8));

  // Spin wheels
  car.wheelSpin += (car.speed * dt) / 0.36;

  // Rear tire dust / water splash particles
  if (car.grounded && Math.abs(car.speed) > 3.5 && Math.random() < (Math.abs(car.speed) / 22) * 0.7) {
    const isLeft = Math.random() < 0.5;
    const px = isLeft ? rlx : rrx;
    const pz = isLeft ? rlz : rrz;
    const py = (isLeft ? yRL : yRR) + 0.18;
    spawnParticle(
      px + (Math.random() - 0.5) * 0.25,
      py,
      pz + (Math.random() - 0.5) * 0.25,
      -sinY * car.speed * 0.15 + (Math.random() - 0.5) * 1.2,
      1.2 + Math.random() * 1.4,
      -cosY * car.speed * 0.15 + (Math.random() - 0.5) * 1.2,
      py < SEA_Y + 0.25
    );
  }

  // Update audio engine pitch
  sound.updateEngine(speedRatio, fwdInput !== 0, false);

  syncVehicleTransform();

  // Check coin pickups
  for (const c of coins) {
    if (c.collected) continue;
    const d = Math.hypot(car.x - c.x, car.z - c.z);
    if (d < 2.85 && Math.abs(car.y + 1.0 - c.y) < 3.0) {
      c.collected = true;
      c.mesh.visible = false;
      collectedCount++;
      if (runTime - lastCoinTime < 8.0) streakCount = Math.min(6, streakCount + 1);
      else streakCount = 0;
      lastCoinTime = runTime;

      const pitch = 1.0 + streakCount * 0.06;
      sound.playOneShot('coin', pitch, 0.92);
      hudCoins.textContent = `${collectedCount} / ${coins.length}`;

      if (collectedCount === coins.length) {
        mode = 'victory';
        victoryStats.textContent = `All ${coins.length} coins collected in ${formatTime(runTime)}!`;
        victoryMenu.classList.remove('hidden');
        hud.classList.add('hidden');
        sound.updateEngine(0, false, true);
      } else if (streakCount >= 2) {
        showToast(`Coin ${collectedCount}/${coins.length} · Trail Streak ×${streakCount + 1}!`, 1.6);
      }
    }
  }

  // Scenic Easter-egg discovery toast near the Red Fox
  if (!foxDiscovered && Math.hypot(car.x - FOX_POS.x, car.z - FOX_POS.y) < 16) {
    foxDiscovered = true;
    showToast('🦊 Discovered the Red Fox Overlook!', 3.2);
  }
}

function syncVehicleTransform() {
  vehicleRoot.position.set(car.x, car.y, car.z);
  vehicleRoot.rotation.order = 'YXZ';
  vehicleRoot.rotation.set(car.pitch, car.yaw, car.roll);
  vehicleBody.rotation.z = car.bodyRoll;

  for (const w of wheelPivots) {
    w.pivot.rotation.y = w.isFront ? car.steer : 0;
    w.spinMesh.rotation.x = car.wheelSpin;
  }

  // Keep directional shadow frustum centered near the player
  sunLight.position.set(car.x - 95, car.y + 130, car.z + 85);
  sunLight.target.position.set(car.x, car.y, car.z);
}

// ============================================================================
// Camera, Minimap & HUD Updates
// ============================================================================
const camPosTarget = new THREE.Vector3();
const camLookTarget = new THREE.Vector3();
let menuOrbitAngle = 0.4;

function updateCamera(dt) {
  if (mode === 'start' || mode === 'loading') {
    menuOrbitAngle += dt * 0.18;
    const rad = 26;
    const focusY = terrainSurface.sample(CABIN_POS.x, CABIN_POS.y + 6) + 2.5;
    camera.position.set(
      CABIN_POS.x + Math.sin(menuOrbitAngle) * rad,
      focusY + 9.5 + Math.sin(menuOrbitAngle * 0.7) * 2.0,
      CABIN_POS.y + 8 + Math.cos(menuOrbitAngle) * rad
    );
    camera.lookAt(CABIN_POS.x, focusY, CABIN_POS.y + 6);
    return;
  }

  const sinY = Math.sin(car.yaw), cosY = Math.cos(car.yaw);
  if (cameraMode === 0) {
    // Dynamic chase camera
    const dist = 8.4 + Math.min(2.2, Math.abs(car.speed) * 0.08);
    camPosTarget.set(car.x - sinY * dist, car.y + 3.9, car.z - cosY * dist);
    const groundAtCam = terrainSurface.sample(camPosTarget.x, camPosTarget.z);
    if (camPosTarget.y < groundAtCam + 1.4) camPosTarget.y = groundAtCam + 1.4;
    camera.position.lerp(camPosTarget, Math.min(1, dt * 6.5));
    camLookTarget.set(car.x + sinY * 4.5, car.y + 1.5, car.z + cosY * 4.5);
    camera.lookAt(camLookTarget);
  } else if (cameraMode === 1) {
    // Hood / driver expedition camera
    camera.position.set(car.x + sinY * 0.45, car.y + 2.05, car.z + cosY * 0.45);
    camera.lookAt(car.x + sinY * 18, car.y + 1.5 - car.pitch * 12, car.z + cosY * 18);
  } else {
    // Scenic photo orbit camera around the 4x4
    menuOrbitAngle += dt * 0.55;
    camera.position.set(
      car.x + Math.sin(menuOrbitAngle) * 10.5,
      car.y + 4.2,
      car.z + Math.cos(menuOrbitAngle) * 10.5
    );
    camera.lookAt(car.x, car.y + 1.2, car.z);
  }
}

function formatTime(sec) {
  const m = Math.floor(sec / 60);
  const s = Math.floor(sec % 60);
  const t = Math.floor((sec % 1) * 10);
  return `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}.${t}`;
}

function updateHud() {
  hudTimer.textContent = formatTime(runTime);
  const kmh = Math.round(Math.abs(car.speed) * 3.6);
  hudSpeed.textContent = kmh;
  const rpmPct = THREE.MathUtils.clamp(12 + (Math.abs(car.speed) / 26) * 88, 12, 100);
  hudRpm.style.width = `${rpmPct.toFixed(0)}%`;

  // Nearest uncollected coin compass
  let best = null;
  let bestDist = Infinity;
  for (const c of coins) {
    if (c.collected) continue;
    const d = Math.hypot(c.x - car.x, c.z - car.z);
    if (d < bestDist) { bestDist = d; best = c; }
  }
  if (best) {
    hudDist.textContent = `${Math.round(bestDist)} m`;
    const worldAngle = Math.atan2(best.x - car.x, best.z - car.z);
    // Relative angle to vehicle forward heading (0 = straight ahead)
    const relDeg = -(worldAngle - car.yaw) * (180 / Math.PI);
    compassArrow.style.transform = `rotate(${relDeg.toFixed(1)}deg)`;
  } else {
    hudDist.textContent = 'ALL DONE!';
  }

  // Draw live minimap
  const w = minimapCanvas.width, h = minimapCanvas.height;
  minimapCtx.drawImage(minimapBg, 0, 0);
  const toMx = (x) => ((x / WORLD_W) + 0.5) * w;
  const toMy = (z) => ((z / WORLD_D) + 0.5) * h;

  // Cabin marker (warm square)
  minimapCtx.fillStyle = '#f5d084';
  minimapCtx.fillRect(toMx(CABIN_POS.x) - 3, toMy(CABIN_POS.y) - 3, 6, 6);

  // Fox marker (orange dot)
  minimapCtx.fillStyle = '#eb7734';
  minimapCtx.beginPath();
  minimapCtx.arc(toMx(FOX_POS.x), toMy(FOX_POS.y), 3, 0, Math.PI * 2);
  minimapCtx.fill();

  // Uncollected coin blips
  for (const c of coins) {
    if (c.collected) continue;
    minimapCtx.fillStyle = c === best ? '#ffffff' : '#ffcf48';
    minimapCtx.beginPath();
    minimapCtx.arc(toMx(c.x), toMy(c.z), c === best ? 3.2 : 2.3, 0, Math.PI * 2);
    minimapCtx.fill();
  }

  // Player heading triangle
  const px = toMx(car.x), py = toMy(car.z);
  minimapCtx.save();
  minimapCtx.translate(px, py);
  // Screen +Y is +Z south, so rotate by -car.yaw + PI
  minimapCtx.rotate(-car.yaw + Math.PI);
  minimapCtx.fillStyle = '#4ff0ff';
  minimapCtx.strokeStyle = '#09131c';
  minimapCtx.lineWidth = 1.5;
  minimapCtx.beginPath();
  minimapCtx.moveTo(0, -6);
  minimapCtx.lineTo(4.5, 5);
  minimapCtx.lineTo(0, 2.5);
  minimapCtx.lineTo(-4.5, 5);
  minimapCtx.closePath();
  minimapCtx.fill();
  minimapCtx.stroke();
  minimapCtx.restore();
}

// ============================================================================
// UI Events & Main Loop
// ============================================================================
async function startPlaying() {
  await sound.start();
  startMenu.classList.add('hidden');
  pauseMenu.classList.add('hidden');
  victoryMenu.classList.add('hidden');
  hud.classList.remove('hidden');
  mode = 'playing';
  showToast('Collect all 18 gold trail coins across the island!', 2.8);
}

function togglePause() {
  if (mode === 'playing') {
    mode = 'paused';
    pauseMenu.classList.remove('hidden');
    hud.classList.add('hidden');
    sound.updateEngine(0, false, true);
  } else if (mode === 'paused') {
    mode = 'playing';
    pauseMenu.classList.add('hidden');
    hud.classList.remove('hidden');
  }
}

function cycleCamera() {
  cameraMode = (cameraMode + 1) % CAM_LABELS.length;
  btnHudCam.textContent = `CAM: ${CAM_LABELS[cameraMode]}`;
}

function toggleMute() {
  sound.muted = !sound.muted;
  sound.applyVolumes();
  btnHudMute.textContent = sound.muted ? '🔇 MUTED' : '🔊 AUDIO';
}

btnPlay.addEventListener('click', startPlaying);
btnResume.addEventListener('click', togglePause);
btnResetCar.addEventListener('click', () => { resetVehicle(); togglePause(); });
btnRestart.addEventListener('click', () => { resetRun(); togglePause(); });
btnFreeroam.addEventListener('click', () => {
  victoryMenu.classList.add('hidden');
  hud.classList.remove('hidden');
  mode = 'playing';
});
btnPlayAgain.addEventListener('click', () => {
  resetRun();
  victoryMenu.classList.add('hidden');
  hud.classList.remove('hidden');
  mode = 'playing';
});
btnHudCam.addEventListener('click', cycleCamera);
btnHudMute.addEventListener('click', toggleMute);
btnHudPause.addEventListener('click', togglePause);

volMusicSlider.addEventListener('input', () => {
  sound.volumes.music = Number(volMusicSlider.value) / 100;
  valMusic.textContent = `${volMusicSlider.value}%`;
  sound.applyVolumes();
});
volAmbSlider.addEventListener('input', () => {
  sound.volumes.amb = Number(volAmbSlider.value) / 100;
  valAmb.textContent = `${volAmbSlider.value}%`;
  sound.applyVolumes();
});
volSfxSlider.addEventListener('input', () => {
  sound.volumes.sfx = Number(volSfxSlider.value) / 100;
  valSfx.textContent = `${volSfxSlider.value}%`;
  sound.applyVolumes();
});

function triggerHorn() {
  const nowSec = performance.now() * 0.001;
  if (nowSec - lastHornTime < 0.24) return;
  lastHornTime = nowSec;
  sound.playOneShot('horn', 0.985 + Math.random() * 0.03, 0.95);
  headlightFlashTimer = 0.34;
  if (foxModel && Math.hypot(car.x - FOX_POS.x, car.z - FOX_POS.y) < 26) {
    foxHopTimer = 0.80;
    showToast('The Red Fox perks up at your horn!', 2.2);
  }
}

window.addEventListener('keydown', (e) => {
  keys[e.code] = true;
  if (e.code === 'Escape' || e.code === 'KeyP') {
    if (mode === 'playing' || mode === 'paused') togglePause();
  } else if (e.code === 'KeyC' && mode === 'playing') {
    cycleCamera();
  } else if (e.code === 'KeyH' && mode === 'playing' && !e.repeat) {
    triggerHorn();
  } else if (e.code === 'KeyR' && mode === 'playing') {
    resetVehicle();
    showToast('Reset 4×4 to cabin yard', 1.4);
  } else if (e.code === 'KeyM' && (mode === 'playing' || mode === 'paused')) {
    toggleMute();
  }
});
window.addEventListener('keyup', (e) => { keys[e.code] = false; });

window.addEventListener('resize', () => {
  camera.aspect = window.innerWidth / window.innerHeight;
  camera.updateProjectionMatrix();
  renderer.setSize(window.innerWidth, window.innerHeight);
});

let lastTime = performance.now();
function animate(now) {
  requestAnimationFrame(animate);
  const dt = Math.min(0.05, (now - lastTime) / 1000);
  lastTime = now;
  const elapsed = now * 0.001;

  // Spin & bob expedition coins
  for (const c of coins) {
    if (c.collected) continue;
    c.coinModel.rotation.y = elapsed * 1.9 + c.phase;
    c.mesh.position.y = c.baseY + Math.sin(elapsed * 2.6 + c.phase) * 0.25;
  }

  // Update dust / splash particles
  for (const p of particles) {
    if (!p.mesh.visible) continue;
    p.life += dt;
    if (p.life >= p.maxLife) {
      p.mesh.visible = false;
      continue;
    }
    p.mesh.position.x += p.vx * dt;
    p.mesh.position.y += p.vy * dt;
    p.mesh.position.z += p.vz * dt;
    p.vy -= 4.0 * dt;
    const t = 1 - p.life / p.maxLife;
    p.mesh.scale.setScalar(t * 0.9);
  }

  if (mode === 'playing') {
    if (collectedCount < coins.length) runTime += dt;
    updateVehicle(dt);
    updateHud();
  }

  if (headlightFlashTimer > 0) {
    headlightFlashTimer = Math.max(0, headlightFlashTimer - dt);
    if (headlight) headlight.intensity = headlightFlashTimer > 0 ? 68 : 22;
  }

  if (foxHopTimer > 0 && foxModel) {
    foxHopTimer = Math.max(0, foxHopTimer - dt);
    const u = 1 - foxHopTimer / 0.80;
    const bounce = Math.sin(u * Math.PI * 2) * (1 - u);
    foxModel.scale.set(1.75 * (1 - bounce * 0.05), 1.75 * (1 + bounce * 0.09), 1.75 * (1 - bounce * 0.05));
    if (foxHopTimer === 0) foxModel.scale.setScalar(1.75);
  }

  if (toastTimer > 0) {
    toastTimer -= dt;
    if (toastTimer <= 0) toastEl.classList.add('hidden');
  }

  updateCamera(dt);
  renderer.render(scene, camera);
}

loadAllAssets().catch((err) => {
  loadingText.textContent = `Error loading demo: ${err.message || err}`;
  console.error(err);
});
requestAnimationFrame(animate);
