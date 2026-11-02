/** Which part of the world is on screen. Positions are in cells, scale is CSS pixels per cell. */
export class Camera {
  x = 0;
  y = 0;
  scale = 8;
  static readonly MIN_SCALE = 0.5;
  static readonly MAX_SCALE = 48;

  centreOn(wx: number, wy: number, el: HTMLElement) {
    this.x = wx - el.clientWidth / 2 / this.scale;
    this.y = wy - el.clientHeight / 2 / this.scale;
  }

  /** Keep the middle of the screen over the world (or the sky above it), so nobody gets lost. */
  clamp(el: HTMLElement, worldW: number, worldH: number) {
    const halfW = el.clientWidth / 2 / this.scale, halfH = el.clientHeight / 2 / this.scale;
    this.x = Math.min(Math.max(this.x, -halfW), worldW - halfW);
    this.y = Math.min(Math.max(this.y, -100 - halfH), worldH - halfH);
  }

  toWorld(sx: number, sy: number): [number, number] {
    return [this.x + sx / this.scale, this.y + sy / this.scale];
  }

  /** Zoom by a factor while keeping the world point under (sx, sy) in place. */
  zoomAt(sx: number, sy: number, factor: number) {
    const [wx, wy] = this.toWorld(sx, sy);
    this.scale = Math.min(Camera.MAX_SCALE, Math.max(Camera.MIN_SCALE, this.scale * factor));
    this.x = wx - sx / this.scale;
    this.y = wy - sy / this.scale;
  }
}

export interface InputHandlers {
  /** A click or tap that was not a drag. */
  tap(sx: number, sy: number): void;
  hover(sx: number | null, sy: number | null): void;
}

/** Mouse and touch: drag to pan, wheel or pinch to zoom, click or tap to act. */
export function attachInput(el: HTMLElement, cam: Camera, h: InputHandlers) {
  const pointers = new Map<number, { x: number; y: number }>();
  let start: { x: number; y: number } | null = null;
  let dragged = false;
  let pinch: { dist: number; mx: number; my: number } | null = null;

  const local = (e: PointerEvent | WheelEvent) => {
    const r = el.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };
  const pinchState = () => {
    const [a, b] = [...pointers.values()];
    return { dist: Math.hypot(a.x - b.x, a.y - b.y), mx: (a.x + b.x) / 2, my: (a.y + b.y) / 2 };
  };

  el.addEventListener("pointerdown", (e) => {
    el.setPointerCapture(e.pointerId);
    const p = local(e);
    pointers.set(e.pointerId, p);
    if (pointers.size === 1) {
      start = p;
      dragged = false;
    } else if (pointers.size === 2) {
      pinch = pinchState();
      dragged = true;
    }
  });

  el.addEventListener("pointermove", (e) => {
    const p = local(e);
    if (e.pointerType === "mouse") h.hover(p.x, p.y);
    const prev = pointers.get(e.pointerId);
    if (!prev) return;
    pointers.set(e.pointerId, p);

    if (pointers.size === 2 && pinch) {
      const now = pinchState();
      cam.zoomAt(now.mx, now.my, now.dist / Math.max(pinch.dist, 1));
      cam.x -= (now.mx - pinch.mx) / cam.scale;
      cam.y -= (now.my - pinch.my) / cam.scale;
      pinch = now;
    } else if (pointers.size === 1 && start) {
      if (!dragged && Math.hypot(p.x - start.x, p.y - start.y) > 6) dragged = true;
      if (dragged) {
        cam.x -= (p.x - prev.x) / cam.scale;
        cam.y -= (p.y - prev.y) / cam.scale;
      }
    }
  });

  const end = (e: PointerEvent) => {
    if (!pointers.has(e.pointerId)) return;
    pointers.delete(e.pointerId);
    if (pointers.size === 0) {
      if (!dragged && start && e.type === "pointerup") h.tap(start.x, start.y);
      start = null;
      pinch = null;
    } else if (pointers.size === 1) {
      // One finger left after a pinch: keep panning from where it is, but never tap.
      pinch = null;
      start = [...pointers.values()][0];
    }
  };
  el.addEventListener("pointerup", end);
  el.addEventListener("pointercancel", end);
  el.addEventListener("pointerleave", (e) => {
    if (e.pointerType === "mouse") h.hover(null, null);
  });

  el.addEventListener("wheel", (e) => {
    e.preventDefault();
    const p = local(e);
    cam.zoomAt(p.x, p.y, Math.exp(-e.deltaY * 0.0015));
  }, { passive: false });
}
