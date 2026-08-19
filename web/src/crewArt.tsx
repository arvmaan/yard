/**
 * Inline crew art (Family A: dispatch tower, orchestrator, worker — and the
 * Family C robot for provider-spawned child agents). Rendered as real DOM
 * SVG, not a background-image, specifically so fills can reference the
 * app's own theme tokens (--muted, --ink, --blue, --yellow, ...) and follow
 * light/dark automatically instead of being baked into a static asset.
 *
 * Each figure is drawn at its natural marker-shell proportions so it reads
 * correctly at 0.78 billboard scale: worker/subagent 80x90, orchestrator
 * 90x102 (~12% more visual mass), tower 112x176. The viewBox on every figure
 * below adds extra empty space above the character (a negative min-y) that
 * the drawn shapes never use — the old sprite PNGs had this same headroom
 * baked into their canvas already; without it here, the character fills the
 * full overflowed sprite box `preserveAspectRatio` scales it into and its
 * head reaches into the label sitting above it.
 */

function ContactShadow({ cx, cy, rx, ry }: { cx: number; cy: number; rx: number; ry: number }) {
  return <ellipse cx={cx} cy={cy} rx={rx} ry={ry} fill="var(--crew-shadow)" />
}

export function WorkerSprite() {
  return (
    <svg
      viewBox="0 -30 80 120"
      role="img"
      aria-hidden="true"
      focusable="false"
      data-crew-role="worker"
    >
      <ContactShadow cx={40} cy={86} rx={26} ry={4} />
      <rect x={30} y={64} width={9} height={22} rx={3} fill="var(--crew-body)" />
      <rect x={44} y={64} width={9} height={22} rx={3} fill="var(--crew-body)" />
      <rect x={27} y={34} width={28} height={34} rx={8} fill="var(--crew-body-soft)" />
      <rect x={34} y={44} width={12} height={10} rx={2} fill="var(--vermilion)" />
      <circle cx={41} cy={20} r={12} fill="var(--crew-body)" />
      <rect x={12} y={46} width={16} height={12} rx={2} fill="var(--crew-body-soft)" />
      <rect x={8} y={52} width={20} height={14} rx={1.5} fill="var(--ink)" fillOpacity={0.72} />
      <rect x={10} y={54} width={16} height={9} fill="var(--blue)" fillOpacity={0.85} />
    </svg>
  )
}

export function OrchestratorSprite() {
  return (
    <svg
      viewBox="0 -34 90 136"
      role="img"
      aria-hidden="true"
      focusable="false"
      data-crew-role="orchestrator"
    >
      <ContactShadow cx={45} cy={98} rx={30} ry={4.5} />
      <rect x={33} y={72} width={10} height={26} rx={3} fill="var(--crew-body)" />
      <rect x={49} y={72} width={10} height={26} rx={3} fill="var(--crew-body)" />
      <path d="M22 40 Q45 28 70 40 L66 78 Q45 86 26 78 Z" fill="var(--crew-body-soft)" />
      <path d="M45 30 L70 40 L64 50 L45 42 Z" fill="var(--yellow)" />
      <circle cx={46} cy={22} r={13} fill="var(--crew-body)" />
      <rect x={16} y={50} width={9} height={16} rx={2} fill="var(--crew-body-soft)" />
      <rect x={12} y={46} width={9} height={11} rx={1.5} fill="var(--ink)" fillOpacity={0.75} />
      <line x1={16} y1={46} x2={16} y2={40} stroke="var(--ink)" strokeWidth={1.4} strokeOpacity={0.75} />
    </svg>
  )
}

export function SuperintendentSprite() {
  return (
    <svg
      viewBox="0 -24 112 200"
      role="img"
      aria-hidden="true"
      focusable="false"
      data-crew-role="superintendent"
    >
      <ContactShadow cx={56} cy={172} rx={40} ry={5} />
      <path d="M16 172 L20 108 L92 108 L96 172 Z" fill="var(--crew-body-soft)" />
      <rect x={26} y={56} width={60} height={56} rx={4} fill="var(--crew-body)" />
      <path d="M20 56 L56 30 L92 56 Z" fill="var(--crew-body-soft)" />
      <rect x={38} y={68} width={36} height={26} rx={2} fill="var(--crew-window)" />
      <circle cx={52} cy={80} r={6} fill="var(--ink)" fillOpacity={0.8} />
      <rect x={44} y={86} width={16} height={8} rx={2} fill="var(--ink)" fillOpacity={0.8} />
      <line x1={56} y1={30} x2={56} y2={8} stroke="var(--muted)" strokeWidth={2} />
      <circle cx={56} cy={8} r={4.5} fill="var(--yellow)" />
    </svg>
  )
}

export function SubagentSprite() {
  return (
    <svg
      viewBox="0 -30 80 120"
      role="img"
      aria-hidden="true"
      focusable="false"
      data-crew-role="subagent"
    >
      <ContactShadow cx={40} cy={84} rx={22} ry={5} />
      <ellipse cx={30} cy={80} rx={7} ry={4} fill="var(--crew-body-soft)" />
      <ellipse cx={50} cy={80} rx={7} ry={4} fill="var(--crew-body-soft)" />
      <rect x={18} y={26} width={44} height={52} rx={16} fill="var(--crew-body)" />
      <rect x={24} y={38} width={32} height={9} rx={4.5} fill="var(--teal)" fillOpacity={0.9} />
      <circle cx={60} cy={52} r={5} fill="var(--teal)" />
      <rect x={8} y={48} width={16} height={12} rx={2} fill="var(--crew-body-soft)" />
      <rect x={4} y={54} width={20} height={14} rx={1} fill="var(--ink)" fillOpacity={0.72} />
      <rect x={6} y={56} width={16} height={9} fill="var(--blue)" fillOpacity={0.85} />
    </svg>
  )
}
