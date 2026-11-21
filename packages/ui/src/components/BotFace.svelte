<script module lang="ts">
  let nextCloudId = 0;
</script>

<script lang="ts">
  // Cloudy's animated cloud face: keeps all live expressions (blinks when idle, looks around while thinking,
  // widens eyes when listening, bounces with voice when speaking, smiles on success) inside a soft white & sky-blue cloud.
  import type { AssistantState } from '../island-layout';

  interface Props {
    mode?: AssistantState;
    level?: number;
    size?: number;
  }

  let { mode = 'idle', level = 0, size = 30 }: Props = $props();

  const gradId = `cloudy-face-${++nextCloudId}`;
  const scale = $derived(mode === 'speaking' ? 1 + Math.min(0.14, level * 0.18) : 1);
</script>

<div class="bot {mode}" style="--size: {size}px; transform: scale({scale});" aria-hidden="true">
  <div class="head">
    <svg class="cloud-svg" viewBox="0 0 40 34" fill="none">
      <defs>
        <linearGradient id={gradId} x1="8" y1="4" x2="32" y2="30" gradientUnits="userSpaceOnUse">
          <stop offset="0%" stop-color="#ffffff" />
          <stop offset="60%" stop-color="#e0f2fe" />
          <stop offset="100%" stop-color="#7dd3fc" />
        </linearGradient>
      </defs>
      <path
        d="M10 29H30C34.8 29 38.5 25.4 38.5 20.8C38.5 16.4 35.1 12.9 30.8 12.6C29.2 7.2 24.4 3.5 18.8 3.5C13 3.5 8.1 7.6 6.8 13.1C3.5 13.9 1.5 17 1.5 20.7C1.5 25.3 5.3 29 10 29Z"
        fill="url(#{gradId})"
        stroke="rgba(255, 255, 255, 0.9)"
        stroke-width="1"
      />
      <path d="M10.5 13.5C12.2 9.2 15.6 6.6 19.6 6.6" stroke="#ffffff" stroke-width="1.6" stroke-linecap="round" opacity="0.85" />
    </svg>
    <div class="eyes">
      <span class="eye left"></span>
      <span class="eye right"></span>
    </div>
  </div>
</div>

<style>
  .bot {
    width: var(--size);
    height: var(--size);
    flex-shrink: 0;
    filter: drop-shadow(0 0 8px var(--glow));
    transition: transform 90ms linear, filter var(--medium) var(--ease-out), width var(--medium) var(--ease-spring),
      height var(--medium) var(--ease-spring);
  }
  .head {
    position: relative;
    width: 100%;
    height: 100%;
    display: grid;
    place-items: center;
  }
  .cloud-svg {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
  }
  @property --look {
    syntax: '<number>';
    inherits: true;
    initial-value: 0;
  }
  @property --phase {
    syntax: '<number>';
    inherits: false;
    initial-value: 0;
  }
  .eyes {
    position: relative;
    z-index: 1;
    display: flex;
    padding-top: 10%;
    gap: calc(var(--size) * 0.13);
    transform: translateX(calc(var(--size) * 0.08 * var(--phase) * var(--look)));
    animation: look 2.2s infinite ease-in-out;
    transition: --look 0.6s var(--ease-out);
  }
  .eye {
    width: calc(var(--size) * 0.12);
    height: calc(var(--size) * 0.28);
    border-radius: 99px;
    background: #0c192c;
    animation: blink 4.6s infinite ease-in-out;
    transition: height var(--medium) var(--ease-spring), border-radius var(--medium), transform var(--medium);
  }
  .left {
    transform: rotate(-6deg);
  }
  .right {
    transform: rotate(6deg);
  }

  .listening .eye {
    height: calc(var(--size) * 0.35);
  }
  .thinking .eyes,
  .working .eyes {
    --look: 1;
  }
  .thinking .eye {
    height: calc(var(--size) * 0.21);
  }
  .success .eye {
    height: calc(var(--size) * 0.1);
    border-radius: 99px 99px 0 0;
    animation: none;
  }
  .error .eye {
    height: calc(var(--size) * 0.09);
    animation: none;
  }
  .error .head {
    animation: shake 0.5s var(--ease-out);
  }

  @keyframes blink {
    0%,
    92%,
    100% {
      scale: 1 1;
    }
    95% {
      scale: 1 0.08;
    }
  }
  @keyframes look {
    0%,
    100% {
      --phase: 0;
    }
    30% {
      --phase: -1;
    }
    70% {
      --phase: 1;
    }
  }
  @keyframes shake {
    0%,
    100% {
      transform: translateX(0);
    }
    25% {
      transform: translateX(-3px);
    }
    75% {
      transform: translateX(3px);
    }
  }
</style>