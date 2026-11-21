<script lang="ts" module>
  // Line icons drawn on a 24 x 24 grid (one path each, stroked with the text colour).
  const circle = (cx: number, cy: number, r: number) => `M${cx - r} ${cy}a${r} ${r} 0 1 0 ${2 * r} 0a${r} ${r} 0 1 0 ${-2 * r} 0`;

  const PATHS = {
    chat: 'M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2zM8 9h8M8 13h5',
    control: 'M12 3a6 6 0 0 0-6 6v6a6 6 0 0 0 12 0V9a6 6 0 0 0-6-6zM12 7v4',
    folder: 'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
    pc: 'M3 5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2zM8 21h8M12 16v5',
    settings: 'M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6',
    back: 'M9 18l6-6-6-6',
    forward: 'M15 18l-6-6 6-6',
    power: 'M12 2v10M18.4 6.6a9 9 0 1 1-12.8 0',
    lock: 'M5 11h14v10H5zM8 11V7a4 4 0 0 1 8 0v4',
    sleep: 'M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z',
    restart: 'M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5',
    wake: 'M13 2L3 14h9l-1 8 10-12h-9l1-8z',
    close: 'M18 6L6 18M6 6l12 12',
    upload: 'M12 16V4M6 10l6-6 6 6M4 20h16',
    download: 'M12 4v12M6 10l6 6 6-6M4 20h16',
    image: `M3 5h18v14H3zM3 16l5-5 4 4 3-3 6 6${circle(8.5, 9.5, 1.5)}`,
    video: 'M3 6h13v12H3zM16 10l5-3v10l-5-3z',
    audio: `M9 18V5l12-2v13${circle(6, 18, 3)}${circle(18, 16, 3)}`,
    doc: 'M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9zM14 3v6h6M8 13h8M8 17h5',
    archive: 'M21 8v13H3V8M1 3h22v5H1zM10 12h4',
    app: 'M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z',
    drive: 'M2 12h20M5.5 5h13l3.5 7v6a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-6zM6 16h.01M10 16h.01',
    file: 'M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9zM14 3v6h6',
    keyboard: 'M2 6h20v12H2zM6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10',
    eye: `M1 12s4-8 11-8 11 8 11 8-4 8-11 8S1 12 1 12z${circle(12, 12, 3)}`,
    copy: 'M9 9h11v11H9zM5 15H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v1',
    send: 'M22 2L11 13M22 2l-7 20-4-9-9-4z',
    mic: 'M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3zM19 10v2a7 7 0 0 1-14 0v-2M12 19v3M8 22h8',
    check: 'M20 6L9 17l-5-5',
    bell: 'M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9M13.7 21a2 2 0 0 1-3.4 0',
    sms: 'M21 11.5a8.4 8.4 0 0 1-9 8.4 8.6 8.6 0 0 1-3.8-.9L3 21l1.9-5.7A8.4 8.4 0 0 1 12 3a8.4 8.4 0 0 1 9 8.5z',
    globe: `${circle(12, 12, 10)}M2 12h20M12 2a15 15 0 0 1 0 20M12 2a15 15 0 0 0 0 20`,
    unlink: 'M18.8 13.4l1.4-1.4a4.5 4.5 0 0 0-6.4-6.4l-1.4 1.4M5.2 10.6l-1.4 1.4a4.5 4.5 0 0 0 6.4 6.4l1.4-1.4M8 2v3M2 8h3M16 22v-3M22 16h-3',
    bug: 'M8 6V4a4 4 0 0 1 8 0v2M6 10h12v5a6 6 0 0 1-12 0zM2 13h4M18 13h4M3 7l3 3M21 7l-3 3M3 20l3-3M21 20l-3-3',
    qr: 'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h3v3h-3zM18 18h3v3h-3zM14 20h1M20 14h1',
    home: 'M3 11l9-8 9 8v10a1 1 0 0 1-1 1h-5v-7h-6v7H4a1 1 0 0 1-1-1z',
    paste: 'M9 3h6v3H9zM8 4H6a1 1 0 0 0-1 1v15a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V5a1 1 0 0 0-1-1h-2',
    wifi: 'M5 12.5a10 10 0 0 1 14 0M8.5 16a5 5 0 0 1 7 0M2 9a15 15 0 0 1 20 0M12 20h.01',
    info: `${circle(12, 12, 10)}M12 16v-4M12 8h.01`,
    refresh: 'M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5',
    volDown: 'M11 5L6 9H2v6h4l5 4zM15.5 8.5a5 5 0 0 1 0 7',
    volUp: 'M11 5L6 9H2v6h4l5 4zM15.5 8.5a5 5 0 0 1 0 7M19 5a9 9 0 0 1 0 14',
    mute: 'M11 5L6 9H2v6h4l5 4zM23 9l-6 6M17 9l6 6',
    playPause: 'M3 5l8 7-8 7zM15 5v14M20 5v14',
    trash: 'M3 6h18M8 6V4h8v2M6 6l1 15h10l1-15',
    pointer: 'M5 3l14 7-6 2-2 6z'
  } as const;

  export type IconName = keyof typeof PATHS;
</script>

<script lang="ts">
  interface Props {
    name: IconName;
    size?: number;
    stroke?: number;
  }

  let { name, size = 22, stroke = 1.9 }: Props = $props();
</script>

<svg
  viewBox="0 0 24 24"
  width={size}
  height={size}
  fill="none"
  stroke="currentColor"
  stroke-width={stroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  <path d={PATHS[name]} />
</svg>

<style>
  svg {
    flex-shrink: 0;
    display: block;
  }
</style>
