import type { WebGLRenderer, CanvasTexture, Mesh } from "three";
import { measureLabel } from "./label-canvas";

// Pinned r180. Imports wait for an observed contentful paint; the still and HTML are already usable.
const intro = document.querySelector<HTMLElement>("#bottle-intro")!;
const stage = intro.querySelector<HTMLElement>(".bottle-stage")!;
const canvas = intro.querySelector("canvas")!;
const page = document.querySelector<HTMLElement>("#label-page")!;
const stillControl = intro.querySelector("button")!;
const chrome = [
  ...intro.querySelectorAll<HTMLElement>(
    ".bottle-nav, .bottle-caption, .bottle-controls",
  ),
];
const motionPreference = matchMedia("(prefers-reduced-motion: reduce)");
const darkPreference = matchMedia("(prefers-color-scheme: dark)");
const capability = navigator as Navigator & {
  deviceMemory?: number;
  connection?: EventTarget & { saveData?: boolean };
};
const connection = capability.connection;
// Hardware hints choose detail, never exclude an iPhone from the experience.
const modestHardware =
  (capability.deviceMemory ?? 8) <= 2 || navigator.hardwareConcurrency <= 2;
const mobile = matchMedia("(max-width: 700px)").matches;
let detail = mobile || modestHardware ? 1 : 0;
let fallbackLogged = false;
let active = false,
  stopped = false,
  draw = () => {},
  resize = () => {},
  dispose = () => {};
let travel = 0,
  requested = false;
const clamp = (value: number) => Math.max(0, Math.min(1, value));
const smooth = (a: number, b: number, value: number) => {
  const t = clamp((value - a) / (b - a));
  return t * t * (3 - 2 * t);
};
function schedule() {
  if (!requested && active && !document.hidden) {
    requested = true;
    requestAnimationFrame(() => {
      requested = false;
      if (active) draw();
    });
  }
}
function fallback(reason: string) {
  intro.dataset.fallback = reason;
  if (!fallbackLogged) {
    console.info(`[rush bottle] fallback: ${reason}`);
    fallbackLogged = true;
  }
  const wasActive = active;
  const oldPageTop = page.getBoundingClientRect().top;
  active = false;
  stopped = true;
  document.documentElement.classList.remove("bottle-active");
  page.style.removeProperty("transform");
  page.style.removeProperty("opacity");
  page.style.removeProperty("visibility");
  page.style.removeProperty("pointer-events");
  page.inert = false;
  canvas.style.opacity = "0";
  canvas.style.visibility = "";
  chrome.forEach((el) => {
    el.style.opacity = "";
    el.style.pointerEvents = "";
  });
  stillControl.hidden = true;
  intro.dataset.mode = reason;
  dispose();
  // Preserve the reading position if the visitor has already reached the HTML.
  if (wasActive && oldPageTop <= 0)
    scrollBy(0, page.getBoundingClientRect().top - oldPageTop);
  else if (wasActive && scrollY > 0) scrollTo(0, page.offsetTop);
}
stillControl.addEventListener("click", () => fallback("still-selected"));
intro.querySelector("a")!.addEventListener("click", (event) => {
  if (!active) return;
  event.preventDefault();
  scrollTo({ top: intro.offsetTop + travel, behavior: "instant" });
  draw();
  page.focus({ preventScroll: true });
});
motionPreference.addEventListener("change", () => {
  if (motionPreference.matches) fallback("reduced-motion");
});
connection?.addEventListener("change", () => {
  if (connection?.saveData) fallback("save-data");
});
canvas.addEventListener("webglcontextlost", (event) => {
  event.preventDefault();
  fallback("context-lost");
});
addEventListener("scroll", schedule, { passive: true });
addEventListener(
  "resize",
  () => {
    if (active) resize();
  },
  { passive: true },
);
document.addEventListener("visibilitychange", schedule);
// Safari toolbar movement changes dvh; this never changes travel or calls scrollTo.
window.visualViewport?.addEventListener(
  "resize",
  () => {
    if (active) resize();
  },
  { passive: true },
);
if (motionPreference.matches || connection?.saveData) {
  fallback(motionPreference.matches ? "reduced-motion" : "save-data");
} else {
  // A second rAF can still precede the browser's first presented paint.
  // Observe FCP where supported, rather than assuming the still has appeared.
  const begin = () => requestAnimationFrame(() => setTimeout(startBottle, 0));
  if (
    performance
      .getEntriesByType("paint")
      .some((entry) => entry.name === "first-contentful-paint")
  )
    begin();
  else if (
    typeof PerformanceObserver !== "undefined" &&
    PerformanceObserver.supportedEntryTypes.includes("paint")
  ) {
    const painted = new PerformanceObserver((list) => {
      if (
        list
          .getEntries()
          .some((entry) => entry.name === "first-contentful-paint")
      ) {
        painted.disconnect();
        begin();
      }
    });
    painted.observe({ type: "paint", buffered: true });
  } else requestAnimationFrame(() => requestAnimationFrame(begin));
}

async function startBottle() {
  if (
    stopped ||
    motionPreference.matches ||
    scrollY >= intro.offsetTop + stage.clientHeight ||
    location.hash
  ) {
    if (!stopped) fallback("already-reading");
    return;
  }
  let renderer: WebGLRenderer | undefined;
  try {
    const context = canvas.getContext("webgl2", {
      alpha: true,
      antialias: true,
    });
    if (!context) {
      fallback("no-webgl");
      return;
    }
    intro.dataset.loading = "module";
    const THREE = await import("three");
    const { RoomEnvironment } =
      await import("three/addons/environments/RoomEnvironment.js");
    if (
      stopped ||
      motionPreference.matches ||
      scrollY >= intro.offsetTop + stage.clientHeight ||
      location.hash
    ) {
      if (!stopped)
        fallback(
          motionPreference.matches ? "reduced-motion" : "already-reading",
        );
      return;
    }
    renderer = new THREE.WebGLRenderer({
      canvas,
      context,
      antialias: true,
      alpha: true,
    });
    const renderRatio = () =>
      Math.min(devicePixelRatio, detail >= 2 ? 1 : detail === 1 ? 1.5 : 2);
    renderer.setPixelRatio(renderRatio());
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    renderer.toneMapping = THREE.ACESFilmicToneMapping;
    renderer.toneMappingExposure = 0.72;
    renderer.setClearColor(0x000000, 0);
    const scene = new THREE.Scene();
    const camera = new THREE.PerspectiveCamera(
      32,
      innerWidth / innerHeight,
      0.1,
      40,
    );
    intro.dataset.loading = "environment";
    // PMREM uses half-float render targets. Keep the bottle if those are absent.
    let environment:
      | ReturnType<InstanceType<typeof THREE.PMREMGenerator>["fromScene"]>
      | undefined;
    if (
      context.getExtension("EXT_color_buffer_float") ||
      context.getExtension("EXT_color_buffer_half_float")
    ) {
      const pmrem = new THREE.PMREMGenerator(renderer);
      const room = new RoomEnvironment();
      environment = pmrem.fromScene(room, 0.025, 0.1, 100, {
        size: detail ? 128 : 256,
      });
      scene.environment = environment.texture;
      scene.environmentIntensity = 0.65;
      room.dispose();
      pmrem.dispose();
      intro.dataset.reflections = "pmrem";
    } else intro.dataset.reflections = "direct-light";
    const key = new THREE.DirectionalLight(0xffffff, 1.65);
    key.position.set(-3, 5, 5);
    scene.add(key);
    const rim = new THREE.DirectionalLight(0xffefbd, 0.8);
    rim.position.set(4, 2, -2);
    scene.add(rim);
    scene.add(new THREE.HemisphereLight(0xffffff, 0x825d05, 0.3));
    const bottle = new THREE.Group();
    scene.add(bottle);
    const shell = new THREE.Group();
    bottle.add(shell);
    const yellow = new THREE.MeshPhysicalMaterial({
      color: 0xffdd00,
      metalness: 0.02,
      roughness: 0.21,
      clearcoat: 1,
      clearcoatRoughness: 0.12,
      ior: 1.48,
      envMapIntensity: 1.35,
    });
    const capMaterial = yellow.clone();
    capMaterial.roughness = 0.35;
    capMaterial.clearcoatRoughness = 0.2;
    // Small squat bottle: broad base, pinched neck and a cap almost as wide as the body.
    const profile = [
      [0, -1.55],
      [0.65, -1.55],
      [0.75, -1.52],
      [0.79, -1.46],
      [0.805, -1.34],
      [0.805, 0.13],
      [0.79, 0.35],
      [0.73, 0.5],
      [0.61, 0.65],
      [0.54, 0.76],
      [0.54, 1.02],
      [0, 1.02],
    ];
    const curvedProfile = new THREE.SplineCurve(
      profile.slice(1, -1).map((p) => new THREE.Vector2(p[0], p[1])),
    ).getPoints(detail ? 64 : 120);
    const bodyPoints = [
      new THREE.Vector2(0, -1.55),
      ...curvedProfile,
      new THREE.Vector2(0, 1.02),
    ];
    const body = new THREE.Mesh(
      new THREE.LatheGeometry(bodyPoints, detail ? 64 : 96),
      yellow,
    );
    shell.add(body);
    const cap = new THREE.Mesh(
      new THREE.CylinderGeometry(0.77, 0.79, 0.77, detail ? 64 : 96, 1),
      capMaterial,
    );
    cap.position.y = 1.12;
    shell.add(cap);
    const lip = new THREE.Mesh(
      new THREE.TorusGeometry(0.755, 0.024, 8, 64),
      capMaterial,
    );
    lip.rotation.x = Math.PI / 2;
    lip.position.y = 1.508;
    shell.add(lip);
    const lowerLip = lip.clone();
    lowerLip.position.y = 0.735;
    shell.add(lowerLip);
    const ridgeGeo = new THREE.CylinderGeometry(0.0075, 0.0075, 0.13, 6);
    const ridges = new THREE.InstancedMesh(ridgeGeo, capMaterial, 64);
    const helper = new THREE.Object3D();
    for (let i = 0; i < 64; i++) {
      const a = (i / 64) * Math.PI * 2;
      helper.position.set(Math.sin(a) * 0.782, 1.415, Math.cos(a) * 0.782);
      helper.updateMatrix();
      ridges.setMatrixAt(i, helper.matrix);
    }
    shell.add(ridges);
    const seamMaterial = yellow.clone();
    seamMaterial.color.setHex(0x9d7500);
    seamMaterial.roughness = 0.4;
    const seam = new THREE.Mesh(
      new THREE.TorusGeometry(0.556, 0.018, 8, 64),
      seamMaterial,
    );
    seam.rotation.x = Math.PI / 2;
    seam.position.y = 0.725;
    shell.add(seam);

    // A feathered contact shadow is the only raster texture apart from the printed label.
    const shadowCanvas = document.createElement("canvas");
    shadowCanvas.width = shadowCanvas.height = 256;
    const shadowContext = shadowCanvas.getContext("2d")!;
    const gradient = shadowContext.createRadialGradient(
      128,
      128,
      10,
      128,
      128,
      126,
    );
    gradient.addColorStop(0, "rgba(39,19,0,.4)");
    gradient.addColorStop(0.35, "rgba(39,19,0,.23)");
    gradient.addColorStop(1, "rgba(39,19,0,0)");
    shadowContext.fillStyle = gradient;
    shadowContext.fillRect(0, 0, 256, 256);
    const shadowTexture = new THREE.CanvasTexture(shadowCanvas);
    const shadowMaterial = new THREE.MeshBasicMaterial({
      map: shadowTexture,
      transparent: true,
      depthWrite: false,
      toneMapped: false,
    });
    const shadow = new THREE.Mesh(
      new THREE.PlaneGeometry(3.2, 2),
      shadowMaterial,
    );
    shadow.rotation.x = -Math.PI / 2;
    shadow.position.set(0.05, -1.565, 0);
    scene.add(shadow);

    // Paper follows the same sampled profile and angular vertices as the body.
    // A 0.2% lift plus polygon offset is enough; no separate sleeve radius.
    const flatInk = { value: 0 };
    const labelMaterial = new THREE.MeshPhysicalMaterial({
      side: THREE.DoubleSide,
      roughness: 0.32,
      metalness: 0.02,
      clearcoat: 0.45,
      clearcoatRoughness: 0.2,
      envMapIntensity: 1.35,
      polygonOffset: true,
      polygonOffsetFactor: -1,
      polygonOffsetUnits: -1,
    });
    labelMaterial.onBeforeCompile = (shader) => {
      shader.uniforms.labelFlat = flatInk;
      shader.fragmentShader =
        "uniform float labelFlat;\n" + shader.fragmentShader;
      shader.fragmentShader = shader.fragmentShader.replace(
        "void main() {",
        `void main() {
        #ifdef USE_MAP
        if (labelFlat >= 1.0) {
          gl_FragColor = linearToOutputTexel(vec4(texture2D(map, vMapUv).rgb, 1.0));
          return;
        }
        #endif
      `,
      );
      shader.fragmentShader = shader.fragmentShader.replace(
        "#include <tonemapping_fragment>",
        "#include <tonemapping_fragment>\n#ifdef USE_MAP\n gl_FragColor.rgb = mix(gl_FragColor.rgb, texture2D(map, vMapUv).rgb, labelFlat);\n#endif",
      );
    };
    const angularSegments = detail ? 64 : 96;
    const arcSteps = detail ? 22 : 32;
    const labelBottom = -1.46,
      labelTop = 0.3;
    const labelRows = [
      labelBottom,
      ...curvedProfile
        .filter((p) => p.y > labelBottom && p.y < labelTop)
        .map((p) => p.y),
      labelTop,
    ];
    const radiusAt = (y: number) => {
      for (let i = 1; i < curvedProfile.length; i++) {
        const a = curvedProfile[i - 1],
          b = curvedProfile[i];
        if (y >= a.y && y <= b.y)
          return (
            THREE.MathUtils.lerp(a.x, b.x, (y - a.y) / (b.y - a.y)) * 1.002
          );
      }
      throw new Error("Label outside body profile");
    };
    const vertices: number[] = [],
      uv: number[] = [],
      indices: number[] = [];
    for (let row = 0; row < labelRows.length; row++) {
      const y = labelRows[row],
        radius = radiusAt(y);
      for (let col = 0; col <= arcSteps; col++) {
        const angle = ((col - arcSteps / 2) * 2 * Math.PI) / angularSegments;
        vertices.push(Math.sin(angle) * radius, y, Math.cos(angle) * radius);
        uv.push(col / arcSteps, (y - labelBottom) / (labelTop - labelBottom));
        if (row && col) {
          const d = row * (arcSteps + 1) + col,
            c = d - 1,
            b = d - arcSteps - 1,
            a = b - 1;
          indices.push(a, b, d, a, d, c);
        }
      }
    }
    const labelGeometry = new THREE.BufferGeometry();
    labelGeometry.setAttribute(
      "position",
      new THREE.Float32BufferAttribute(vertices, 3),
    );
    labelGeometry.setAttribute("uv", new THREE.Float32BufferAttribute(uv, 2));
    labelGeometry.setIndex(indices);
    labelGeometry.computeVertexNormals();
    const wrappedPositions = Float32Array.from(vertices);
    const positions = labelGeometry.attributes.position;
    const uvs = labelGeometry.attributes.uv;
    const label = new THREE.Mesh(labelGeometry, labelMaterial);
    bottle.add(label);
    let width = 0,
      height = 0,
      textureVersion = 0,
      htmlTexture: CanvasTexture | null = null,
      pixelTexture: CanvasTexture | null = null;
    let sampleFrames = 0,
      slowFrames = 0,
      warmUntil = Infinity;
    let uploading = false,
      paintedWidth = 0;
    const resetBudget = () => {
      sampleFrames = 0;
      slowFrames = 0;
      warmUntil = performance.now() + 1000;
    };
    let baseDistance = 8.2,
      flatWidth = 5,
      flatHeight = 3;
    let lastProgress = -1;
    const look = new THREE.Vector3();
    let flatY = 0;
    function updateVertices(unroll: number) {
      for (let i = 0; i < positions.count; i++) {
        const u = uvs.getX(i),
          v = uvs.getY(i);
        positions.setXYZ(
          i,
          THREE.MathUtils.lerp(
            wrappedPositions[i * 3],
            (u - 0.5) * flatWidth,
            unroll,
          ),
          THREE.MathUtils.lerp(
            wrappedPositions[i * 3 + 1],
            flatY + (v - 0.5) * flatHeight,
            unroll,
          ),
          THREE.MathUtils.lerp(wrappedPositions[i * 3 + 2], 0, unroll),
        );
      }
      positions.needsUpdate = true;
      labelGeometry.computeVertexNormals();
      labelGeometry.computeBoundingSphere();
      flatInk.value = smooth(0.3, 1, unroll);
    }
    const designTokens = getComputedStyle(document.documentElement);
    const designWidth = Number(
      designTokens.getPropertyValue("--label-design-width"),
    );
    const designHeight = Number(
      designTokens.getPropertyValue("--label-design-height"),
    );
    async function htmlSnapshot() {
      const version = ++textureVersion;
      uploading = true;
      try {
        const face = page.querySelector<HTMLElement>("[data-label-face]")!;
        const art = face.getBoundingClientRect();
        const paint = await measureLabel(face, art.width, art.height);
        if (stopped || version !== textureVersion) return;
        const limit = Math.min(
          renderer!.capabilities.maxTextureSize,
          detail >= 2 ? 1024 : detail ? 2048 : 4096,
        );
        const scale = Math.min(
          limit / Math.max(designWidth, designHeight),
          detail >= 2 ? 1 : 2,
        );
        const textureCanvas = paint((scale * designWidth) / art.width);
        const texture = new THREE.CanvasTexture(textureCanvas);
        texture.colorSpace = THREE.SRGBColorSpace;
        texture.anisotropy = Math.min(
          detail ? 2 : 4,
          renderer!.capabilities.getMaxAnisotropy(),
        );
        renderer!.initTexture(texture);
        htmlTexture?.dispose();
        htmlTexture = texture;
        labelMaterial.map = texture;
        labelMaterial.needsUpdate = true;
        const pixels = paint(
          Math.min(
            devicePixelRatio,
            2,
            limit / Math.max(art.width, art.height),
          ),
        );
        const native = new THREE.CanvasTexture(pixels);
        native.colorSpace = THREE.SRGBColorSpace;
        native.generateMipmaps = false;
        native.minFilter = THREE.LinearFilter;
        native.magFilter = THREE.LinearFilter;
        renderer!.initTexture(native);
        pixelTexture?.dispose();
        pixelTexture = native;
        intro.dataset.texture = `${textureCanvas.width}x${textureCanvas.height}`;
        intro.dataset.painter = "canvas-2d";
        paintedWidth = art.width;
        lastProgress = -1;
      } finally {
        if (version === textureVersion) {
          uploading = false;
          resetBudget();
          schedule();
        }
      }
    }
    const measureStage = () => {
      if (
        stage.clientWidth === width &&
        stage.clientHeight === height &&
        travel
      )
        return;
      const widthChanged = stage.clientWidth !== width;
      const previousTravel = travel,
        previousProgress = previousTravel
          ? clamp((scrollY - intro.offsetTop) / previousTravel)
          : 0;
      const beyond = previousTravel
        ? Math.max(0, scrollY - intro.offsetTop - previousTravel)
        : 0;
      width = stage.clientWidth;
      height = stage.clientHeight;
      renderer!.setSize(width, height, false);
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
      if (!travel || widthChanged)
        travel = Math.round(height * (width < 700 ? 1.75 : 2.5));
      document.documentElement.style.setProperty(
        "--bottle-travel",
        travel + "px",
      );
      document.documentElement.style.setProperty(
        "--bottle-height",
        height + "px",
      );
      if (previousTravel && active && widthChanged)
        scrollTo(0, intro.offsetTop + previousProgress * travel + beyond);
      const viewAngle = THREE.MathUtils.degToRad(camera.fov / 2);
      baseDistance = Math.max(
        8.2,
        1.18 / (Math.tan(viewAngle) * camera.aspect),
      );
      if (width < 700 && height < 700) baseDistance *= 1.35;
      const viewHeight = 2 * Math.tan(viewAngle) * 3.1;
      const frame = page
        .querySelector<HTMLElement>(".label-frame")!
        .getBoundingClientRect();
      flatHeight = (viewHeight * frame.height) / height;
      flatWidth = (viewHeight * frame.width) / height;
      const frameTop = Math.max(0, (height - frame.height) / 2);
      flatY =
        viewHeight / 2 - ((frameTop + frame.height / 2) * viewHeight) / height;
      lastProgress = -1;
      resetBudget();
      schedule();
    };
    resize = () => {
      measureStage();
      const artWidth = page
        .querySelector<HTMLElement>(".label-frame")!
        .getBoundingClientRect().width;
      if (artWidth !== paintedWidth)
        htmlSnapshot().catch((error: unknown) => {
          intro.dataset.failure = String(error);
          fallback("texture-unavailable");
        });
    };
    draw = () => {
      const p = clamp((scrollY - intro.offsetTop) / travel);
      if (p === lastProgress) return;
      lastProgress = p;
      const start = performance.now();
      const straighten = smooth(0.22, 0.64, p),
        zoom = smooth(0.18, 0.64, p),
        unroll = smooth(0.54, 0.88, p);
      const turn = -0.18 + smooth(0, 0.25, p) * 0.42;
      bottle.rotation.set(
        THREE.MathUtils.lerp(0.04, 0, straighten),
        THREE.MathUtils.lerp(turn, 0, straighten),
        THREE.MathUtils.lerp(-0.075, 0, straighten),
      );
      bottle.position.set(
        0,
        width < 700
          ? THREE.MathUtils.lerp(height < 700 ? 0.85 : 0.34, 0, zoom)
          : 0,
        0,
      );
      camera.position.set(
        0,
        THREE.MathUtils.lerp(0.45, 0, zoom),
        THREE.MathUtils.lerp(baseDistance, 3.1, zoom),
      );
      look.set(0, THREE.MathUtils.lerp(-0.05, 0, zoom), 0);
      camera.lookAt(look);
      // Keep fill rate bounded throughout, including the last flat frames.
      const pixelRatio =
        unroll === 1 ? Math.min(devicePixelRatio, 2) : renderRatio();
      if (renderer!.getPixelRatio() !== pixelRatio)
        renderer!.setPixelRatio(pixelRatio);
      updateVertices(unroll);
      labelMaterial.map = unroll === 1 ? pixelTexture : htmlTexture;
      shell.visible = p < 0.54;
      shadowMaterial.opacity = 1 - smooth(0.15, 0.55, p);

      const interfaceOpacity = 1 - smooth(0.07, 0.23, p);
      chrome.forEach((el) => {
        el.style.opacity = String(interfaceOpacity);
        el.style.pointerEvents = interfaceOpacity > 0.5 ? "" : "none";
      });
      // Hold the same flat image for the last 12% of travel, then swap atomically.
      // The page starts at precisely travel; there is no opacity or position tween.
      const showing = p >= 1;
      page.style.visibility = showing ? "visible" : "hidden";
      page.inert = !showing;
      page.style.pointerEvents = showing ? "" : "none";
      canvas.style.visibility = showing ? "hidden" : "visible";
      intro.dataset.progress = p.toFixed(4);
      intro.dataset.phase =
        p < 0.22 ? "turn" : p < 0.54 ? "approach" : showing ? "html" : "unwrap";
      if (!showing) renderer!.render(scene, camera);
      const cost = performance.now() - start;
      // Upload/compile/resize are not steady-state frames. Lower detail first.
      if (
        !uploading &&
        performance.now() > warmUntil &&
        p > 0 &&
        p < 0.88 &&
        ++sampleFrames > 30
      ) {
        slowFrames = cost > 50 ? slowFrames + 1 : Math.max(0, slowFrames - 1);
        if (slowFrames > 30 && detail < 2) {
          detail++;
          intro.dataset.quality = String(detail);
          renderer!.setPixelRatio(renderRatio());
          htmlSnapshot().catch(() => fallback("texture-unavailable"));
        } else if (slowFrames > 90 && detail === 2 && cost > 100)
          fallback("low-frame-budget");
      }
    };
    dispose = () => {
      scene.traverse((object) => {
        if ((object as Mesh).isMesh) (object as Mesh).geometry.dispose();
      });
      for (const material of [
        yellow,
        capMaterial,
        seamMaterial,
        shadowMaterial,
        labelMaterial,
      ])
        material.dispose();
      darkPreference.removeEventListener("change", repaintTheme);
      htmlTexture?.dispose();
      pixelTexture?.dispose();
      shadowTexture.dispose();
      environment?.dispose();
      renderer!.dispose();
    };
    const repaintTheme = () => {
      if (!active) return;
      lastProgress = -1;
      htmlSnapshot().catch((error: unknown) => {
        intro.dataset.failure = String(error);
        fallback("texture-unavailable");
      });
      schedule();
    };
    darkPreference.addEventListener("change", repaintTheme);
    width = stage.clientWidth;
    height = stage.clientHeight;
    renderer!.setSize(width, height, false);
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
    measureStage();
    intro.dataset.loading = "label";
    await htmlSnapshot();
    if (stopped || scrollY >= intro.offsetTop + stage.clientHeight) {
      dispose();
      if (!stopped) fallback("already-reading");
      return;
    }
    intro.dataset.loading = "shaders";
    // Explicitly upload both textures and render the mapped shader before reveal.
    // compileAsync alone does not upload textures or warm the GPU draw path.
    updateVertices(0);
    camera.position.set(0, 0.45, 8.2);
    camera.lookAt(0, -0.05, 0);
    await renderer.compileAsync(scene, camera);
    if (stopped) return;
    renderer.initTexture(shadowTexture);
    renderer.render(scene, camera);
    labelMaterial.map = pixelTexture;
    renderer.render(scene, camera);
    labelMaterial.map = htmlTexture;
    resetBudget();
    if (stopped || scrollY >= intro.offsetTop + stage.clientHeight) {
      dispose();
      if (!stopped) fallback("already-reading");
      return;
    }
    active = true;
    intro.dataset.mode = "webgl";
    intro.dataset.loading = "ready";
    intro.dataset.quality = String(detail);
    delete intro.dataset.fallback;
    document.documentElement.classList.add("bottle-active");
    stillControl.hidden = false;
    draw();
  } catch (error) {
    if (stopped) return;
    intro.dataset.failure =
      error instanceof Error ? error.message : String(error);
    renderer?.dispose();
    fallback("unavailable");
  }
}
