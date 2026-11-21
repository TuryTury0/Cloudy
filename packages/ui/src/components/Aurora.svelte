<script lang="ts">
  // Flowing glow drawn with a small WebGL shader: the theme colours drift like an aurora and brighten with the
  // voice level. Falls back to a CSS gradient when WebGL is not available.
  //
  // Every change is eased, never jumped: colours, brightness and speed glide to their new values, and time is
  // accumulated (time += dt * speed) so a speed change never teleports the pattern. The glow is soft, so it is
  // drawn at low resolution and stretched, which keeps it cheap while the island morphs.
  import { onMount } from 'svelte';

  interface Props {
    colors?: string[];
    level?: number;
    intensity?: number;
    speed?: number;
  }

  let { colors = ['#ffffff', '#94a3b8', '#1e293b'], level = 0, intensity = 1, speed = 1 }: Props = $props();

  let canvas: HTMLCanvasElement;
  let fallback = $state(false);

  const VERTEX = `
    attribute vec2 aPos;
    void main() { gl_Position = vec4(aPos, 0.0, 1.0); }
  `;

  const FRAGMENT = `
    precision mediump float;
    uniform vec2 uRes;
    uniform float uTime;
    uniform vec3 uC0;
    uniform vec3 uC1;
    uniform vec3 uC2;
    uniform float uLevel;
    uniform float uIntensity;

    float hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
    float noise(vec2 p) {
      vec2 i = floor(p);
      vec2 f = fract(p);
      f = f * f * (3.0 - 2.0 * f);
      return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), f.x), f.y);
    }
    float fbm(vec2 p) {
      float v = 0.0;
      float a = 0.5;
      for (int i = 0; i < 4; i++) { v += a * noise(p); p *= 2.0; a *= 0.5; }
      return v;
    }
    void main() {
      vec2 uv = gl_FragCoord.xy / uRes;
      float t = uTime * 0.12;
      float n1 = fbm(uv * vec2(3.0, 1.4) + vec2(t, -t * 0.6));
      float n2 = fbm(uv * vec2(2.0, 2.4) - vec2(t * 0.8, t * 0.3) + n1);
      vec3 col = mix(uC0, uC1, smoothstep(0.2, 0.8, n1));
      col = mix(col, uC2, smoothstep(0.35, 0.9, n2) * 0.8);
      float glow = 0.35 + 0.65 * smoothstep(0.1, 0.9, n2) + uLevel * 0.6;
      float alpha = clamp(glow * uIntensity, 0.0, 1.0);
      gl_FragColor = vec4(col * alpha, alpha);
    }
  `;

  /** Glow resolution relative to CSS pixels; the shader output is smooth, so a quarter of the pixels is plenty. */
  const RENDER_SCALE = 0.5;
  /** Seconds for an eased value to get most of the way (about 63%) to its target. */
  const EASE_SECONDS = 0.35;

  function hexToVec(hex: string): [number, number, number] {
    let h = hex.trim().replace('#', '');
    if (h.length === 3) h = [...h].map((c) => c + c).join('');
    const n = parseInt(h.padEnd(6, '0').slice(0, 6), 16) || 0;
    return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
  }

  function palette(list: string[]): string[] {
    return [list[0] ?? '#ffffff', list[1] ?? list[0] ?? '#ffffff', list[2] ?? list[1] ?? list[0] ?? '#ffffff'];
  }

  onMount(() => {
    const gl = canvas.getContext('webgl', { alpha: true, premultipliedAlpha: true, antialias: false });
    if (!gl) {
      fallback = true;
      return;
    }
    const compile = (type: number, source: string) => {
      const shader = gl.createShader(type)!;
      gl.shaderSource(shader, source);
      gl.compileShader(shader);
      return shader;
    };
    const program = gl.createProgram()!;
    gl.attachShader(program, compile(gl.VERTEX_SHADER, VERTEX));
    gl.attachShader(program, compile(gl.FRAGMENT_SHADER, FRAGMENT));
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      fallback = true;
      return;
    }
    gl.useProgram(program);
    const buffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const aPos = gl.getAttribLocation(program, 'aPos');
    gl.enableVertexAttribArray(aPos);
    gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);

    const u = (name: string) => gl.getUniformLocation(program, name);
    const uRes = u('uRes');
    const uTime = u('uTime');
    const uC = [u('uC0'), u('uC1'), u('uC2')];
    const uLevel = u('uLevel');
    const uIntensity = u('uIntensity');

    const resize = () => {
      const w = Math.max(1, Math.round(canvas.clientWidth * RENDER_SCALE));
      const h = Math.max(1, Math.round(canvas.clientHeight * RENDER_SCALE));
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w;
        canvas.height = h;
        gl.viewport(0, 0, w, h);
      }
    };

    let frame = 0;
    let last = performance.now();
    // Eased values start at their targets (no fade-in from black); a random start time keeps windows from
    // showing the exact same pattern.
    let time = Math.random() * 100;
    let curSpeed = speed;
    let curIntensity = intensity;
    let curLevel = 0;
    let targetKey = '';
    let target = palette(colors).map(hexToVec);
    const current = target.map((c) => [...c]);
    const draw = (now: number) => {
      const dt = Math.min(0.1, Math.max(0, (now - last) / 1000));
      last = now;
      const ease = 1 - Math.exp(-dt / EASE_SECONDS);
      resize();
      const key = colors.join(',');
      if (key !== targetKey) {
        targetKey = key;
        target = palette(colors).map(hexToVec);
      }
      for (let i = 0; i < 3; i++) {
        for (let k = 0; k < 3; k++) current[i][k] += (target[i][k] - current[i][k]) * ease;
        gl.uniform3fv(uC[i], current[i]);
      }
      curSpeed += (speed - curSpeed) * ease;
      curIntensity += (intensity - curIntensity) * ease;
      curLevel += (level - curLevel) * (1 - Math.exp(-dt / 0.08));
      // With "Reduce motion" the glow keeps its colours but stops flowing.
      if (!document.documentElement.classList.contains('reduce-motion')) time += dt * curSpeed;
      gl.uniform2f(uRes, canvas.width, canvas.height);
      gl.uniform1f(uTime, time);
      gl.uniform1f(uLevel, curLevel);
      gl.uniform1f(uIntensity, curIntensity);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      frame = requestAnimationFrame(draw);
    };
    frame = requestAnimationFrame(draw);

    return () => {
      cancelAnimationFrame(frame);
      gl.getExtension('WEBGL_lose_context')?.loseContext();
    };
  });
</script>

<canvas bind:this={canvas} class="aurora" class:hidden={fallback} aria-hidden="true"></canvas>
{#if fallback}
  <div
    class="aurora fallback"
    aria-hidden="true"
    style="background: linear-gradient(100deg, {colors[0]}, {colors[1] ?? colors[0]}, {colors[2] ?? colors[0]}); opacity: {0.5 * intensity};"
  ></div>
{/if}

<style>
  .aurora {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
    border-radius: inherit;
  }
  .hidden {
    display: none;
  }
  .fallback {
    filter: blur(18px);
    animation: drift 8s ease-in-out infinite alternate;
  }
  @keyframes drift {
    from {
      transform: translateX(-8%);
    }
    to {
      transform: translateX(8%);
    }
  }
</style>
