import type { WebGLRenderer, CanvasTexture, Mesh } from "three";

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
const constrained =
  connection?.saveData ||
  (capability.deviceMemory && capability.deviceMemory <= 2) ||
  navigator.hardwareConcurrency <= 2;
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
if (motionPreference.matches || constrained) {
  intro.dataset.mode = motionPreference.matches
    ? "reduced-motion"
    : "low-power";
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
  if (stopped || motionPreference.matches || scrollY > 48 || location.hash) {
    if (!stopped) intro.dataset.mode = "reading";
    return;
  }
  let renderer: WebGLRenderer | undefined;
  try {
    const context = canvas.getContext("webgl2", {
      alpha: true,
      antialias: true,
      powerPreference: "low-power",
      preserveDrawingBuffer: true,
      failIfMajorPerformanceCaveat: true,
    });
    if (!context) {
      fallback("no-webgl");
      return;
    }
    intro.dataset.loading = "module";
    const THREE = await import("three");
    const { RoomEnvironment } = await import(
      "three/addons/environments/RoomEnvironment.js"
    );
    if (stopped || motionPreference.matches || scrollY > 48 || location.hash) {
      intro.dataset.mode = "reading";
      return;
    }
    renderer = new THREE.WebGLRenderer({
      canvas,
      context,
      antialias: true,
      alpha: true,
      powerPreference: "low-power",
      preserveDrawingBuffer: true,
    });
    renderer.setPixelRatio(
      Math.min(devicePixelRatio, innerWidth < 700 ? 1.5 : 1.75),
    );
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
    const pmrem = new THREE.PMREMGenerator(renderer);
    const room = new RoomEnvironment();
    const environment = pmrem.fromScene(room, 0.025);
    scene.environment = environment.texture;
    scene.environmentIntensity = 0.65;
    room.dispose();
    pmrem.dispose();
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
    ).getPoints(160);
    const bodyPoints = [
      new THREE.Vector2(0, -1.55),
      ...curvedProfile,
      new THREE.Vector2(0, 1.02),
    ];
    const body = new THREE.Mesh(
      new THREE.LatheGeometry(bodyPoints, 128),
      yellow,
    );
    shell.add(body);
    const cap = new THREE.Mesh(
      new THREE.CylinderGeometry(0.77, 0.79, 0.77, 128, 1),
      capMaterial,
    );
    cap.position.y = 1.12;
    shell.add(cap);
    const lip = new THREE.Mesh(
      new THREE.TorusGeometry(0.755, 0.024, 12, 128),
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
      new THREE.TorusGeometry(0.556, 0.018, 8, 128),
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

    // The texture is the real responsive label. There is no second bottle artwork.
    const labelMaterial = new THREE.MeshBasicMaterial({
      side: THREE.DoubleSide,
      toneMapped: false,
    });
    const labelGeometry = new THREE.PlaneGeometry(1, 1, 160, 60);
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
      slowFrames = 0;
    let baseDistance = 8.2,
      flatWidth = 5,
      flatHeight = 3;
    let lastProgress = -1;
    const look = new THREE.Vector3();
    const radiusAt = (y: number) => {
      if (y > 0.735) return 0.804 - (y - 0.735) * 0.026;
      for (let j = 1; j < curvedProfile.length; j++) {
        const lo = curvedProfile[j - 1],
          hi = curvedProfile[j];
        if (y >= lo.y && y <= hi.y)
          return (
            THREE.MathUtils.lerp(lo.x, hi.x, (y - lo.y) / (hi.y - lo.y)) + 0.012
          );
      }
      return 0.817;
    };
    function updateVertices(unroll: number) {
      for (let i = 0; i < positions.count; i++) {
        const u = uvs.getX(i),
          v = uvs.getY(i),
          angle = (u - 0.5) * 2.8,
          y = -1.42 + v * 1.88,
          r = radiusAt(y);
        const x = Math.sin(angle) * r,
          z = Math.cos(angle) * r;
        positions.setXYZ(
          i,
          THREE.MathUtils.lerp(x, (u - 0.5) * flatWidth, unroll),
          THREE.MathUtils.lerp(y, (v - 0.5) * flatHeight, unroll),
          THREE.MathUtils.lerp(z, 0, unroll),
        );
      }
      positions.needsUpdate = true;
      labelGeometry.computeVertexNormals();
      labelGeometry.computeBoundingSphere();
    }
    async function htmlSnapshot() {
      const version = ++textureVersion;
      await document.fonts.ready;
      const snapshot = document.createElement("div");
      snapshot.setAttribute("xmlns", "http://www.w3.org/1999/xhtml");
      snapshot.setAttribute(
        "style",
        "width:" +
          width +
          "px;height:" +
          height +
          "px;overflow:hidden;background:var(--ground);color:var(--print);font:400 var(--body)/1.6 var(--sans);-webkit-font-smoothing:antialiased;",
      );
      // Same-origin Vite CSS is already loaded; no duplicate CSS download or separate layout.
      const style = document.createElement("style");
      style.textContent = Array.from(document.styleSheets)
        .map((sheet) =>
          Array.from(sheet.cssRules)
            .map((rule) => rule.cssText)
            .join("\n"),
        )
        .join("\n");
      snapshot.append(style);
      // SVG image documents don't inherit the host's dark preference consistently.
      // Resolve every token from the live page, including the actual system theme.
      const tokens = getComputedStyle(document.documentElement);
      for (const name of Array.from(tokens))
        if (name.startsWith("--"))
          snapshot.style.setProperty(name, tokens.getPropertyValue(name));
      snapshot.append(page.querySelector("[data-label-face]")!.cloneNode(true));
      const serial = new XMLSerializer().serializeToString(snapshot);
      const svg =
        '<svg xmlns="http://www.w3.org/2000/svg" width="' +
        width +
        '" height="' +
        height +
        '"><foreignObject width="100%" height="100%">' +
        serial +
        "</foreignObject></svg>";
      const image = new Image();
      image.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
      await image.decode();
      if (stopped || version !== textureVersion) return;
      const textureCanvas = document.createElement("canvas");
      const scale = Math.min(
        4096 / Math.max(width, height),
        Math.max(2, 2048 / Math.max(width, height)),
      );
      textureCanvas.width = Math.round(width * scale);
      textureCanvas.height = Math.round(height * scale);
      textureCanvas
        .getContext("2d")!
        .drawImage(image, 0, 0, textureCanvas.width, textureCanvas.height);
      // Fail closed if foreignObject cannot be used as a texture on this browser.
      textureCanvas.getContext("2d")!.getImageData(0, 0, 1, 1);
      const texture = new THREE.CanvasTexture(textureCanvas);
      texture.colorSpace = THREE.SRGBColorSpace;
      texture.anisotropy = Math.min(
        8,
        renderer!.capabilities.getMaxAnisotropy(),
      );
      htmlTexture?.dispose();
      htmlTexture = texture;
      labelMaterial.map = texture;
      labelMaterial.needsUpdate = true;
      // A native-resolution level of the SAME SVG avoids resampling glyph edges at the swap.
      const pixels = document.createElement("canvas");
      pixels.width = Math.floor(width * devicePixelRatio);
      pixels.height = Math.floor(height * devicePixelRatio);
      if (Math.max(pixels.width, pixels.height) > renderer!.capabilities.maxTextureSize) throw new Error("Viewport exceeds texture size");
      pixels
        .getContext("2d")!
        .drawImage(image, 0, 0, pixels.width, pixels.height);
      pixelTexture?.dispose();
      pixelTexture = new THREE.CanvasTexture(pixels);
      pixelTexture.colorSpace = THREE.SRGBColorSpace;
      pixelTexture.generateMipmaps = false;
      pixelTexture.minFilter = THREE.NearestFilter;
      pixelTexture.magFilter = THREE.NearestFilter;
      intro.dataset.texture = textureCanvas.width + "x" + textureCanvas.height;
      lastProgress = -1;
      schedule();
    }
    resize = () => {
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
      travel = height * (width < 700 ? 1.75 : 2.5);
      document.documentElement.style.setProperty(
        "--bottle-travel",
        travel + "px",
      );
      document.documentElement.style.setProperty(
        "--bottle-height",
        height + "px",
      );
      if (previousTravel && active)
        scrollTo(0, intro.offsetTop + previousProgress * travel + beyond);
      const viewAngle = THREE.MathUtils.degToRad(camera.fov / 2);
      baseDistance = Math.max(
        8.2,
        1.18 / (Math.tan(viewAngle) * camera.aspect),
      );
      if (width < 700 && height < 700) baseDistance *= 1.35;
      flatHeight = 2 * Math.tan(viewAngle) * 3.1;
      flatWidth = flatHeight * camera.aspect;
      lastProgress = -1;
      htmlSnapshot().catch(() => fallback("texture-unavailable"));
      schedule();
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
        width < 700 ? THREE.MathUtils.lerp(height < 700 ? 0.85 : 0.34, 0, zoom) : 0,
        0,
      );
      camera.position.set(
        0,
        THREE.MathUtils.lerp(0.45, 0, zoom),
        THREE.MathUtils.lerp(baseDistance, 3.1, zoom),
      );
      look.set(0, THREE.MathUtils.lerp(-0.05, 0, zoom), 0);
      camera.lookAt(look);
      // Curved frames have a bounded fill rate; flat frames use actual screen pixels
      // so a Retina display does not resample text at the HTML handoff.
      const pixelRatio = unroll === 1 ? devicePixelRatio : Math.min(devicePixelRatio, width < 700 ? 1.5 : 1.75);
      if (renderer!.getPixelRatio() !== pixelRatio) renderer!.setPixelRatio(pixelRatio);
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
      // Ignore shader warm-up. Sustained expensive frames fall back instead of draining a weak device.
      if (p > 0 && p < 0.9 && ++sampleFrames > 10) {
        slowFrames = cost > 50 ? slowFrames + 1 : Math.max(0, slowFrames - 1);
        if (slowFrames > 20) fallback("low-frame-budget");
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
      environment.dispose();
      renderer!.dispose();
    };
    const repaintTheme = () => {
      if (!active) return;
      lastProgress = -1;
      htmlSnapshot().catch(() => fallback("texture-unavailable"));
      schedule();
    };
    darkPreference.addEventListener("change", repaintTheme);
    width = stage.clientWidth;
    height = stage.clientHeight;
    renderer!.setSize(width, height, false);
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
    intro.dataset.loading = "label";
    await htmlSnapshot();
    if (stopped || scrollY > 48) {
      dispose();
      if (!stopped) intro.dataset.mode = "reading";
      return;
    }
    intro.dataset.loading = "shaders";
    await renderer.compileAsync(scene, camera);
    if (stopped || scrollY > 48) {
      dispose();
      if (!stopped) intro.dataset.mode = "reading";
      return;
    }
    active = true;
    intro.dataset.mode = "webgl";
    intro.dataset.loading = "ready";
    document.documentElement.classList.add("bottle-active");
    stillControl.hidden = false;
    resize();
    draw();
  } catch (error) {
    intro.dataset.failure =
      error instanceof Error ? error.message : String(error);
    renderer?.dispose();
    fallback("unavailable");
  }
}
