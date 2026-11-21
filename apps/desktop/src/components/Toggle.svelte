<script lang="ts">
  interface Props {
    checked: boolean;
    onchange: (checked: boolean) => void;
    label?: string;
  }

  let { checked, onchange, label = '' }: Props = $props();
</script>

<button type="button" role="switch" aria-checked={checked} aria-label={label} class="toggle" class:on={checked} onclick={() => onchange(!checked)}>
  <span class="knob"></span>
</button>

<style>
  .toggle {
    position: relative;
    width: 44px;
    height: 24px;
    border-radius: 99px;
    border: 1px solid var(--border);
    background: rgba(255, 255, 255, 0.08);
    cursor: pointer;
    transition: background var(--medium) var(--ease-out);
    direction: ltr;
  }
  .on {
    background: var(--accent-text);
  }
  .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 2px 6px rgba(0, 0, 0, 0.4);
    transition: transform var(--medium) var(--ease-spring);
  }
  .on .knob {
    transform: translateX(20px);
    background: #0b0f19;
  }
</style>
