"use client";

import { useMemo, useState, useCallback, useId } from "react";

export interface Series {
  key: string;
  label: string;
  color: string;
  values: number[];
  dashed?: boolean;
}

export interface TimeSeriesChartProps {
  title?: string;
  width?: number;
  height?: number;
  yLabel?: string;
  yMin?: number;
  yMax?: number;
  timestamps: number[];
  series: Series[];
}

const PAD = { top: 12, right: 16, bottom: 36, left: 48 };

export function TimeSeriesChart({
  title,
  width = 800,
  height = 280,
  yLabel,
  yMin: forcedYMin,
  yMax: forcedYMax,
  timestamps,
  series,
}: TimeSeriesChartProps) {
  const uid = useId().replace(/:/g, "");
  const plotW = width - PAD.left - PAD.right;
  const plotH = height - PAD.top - PAD.bottom;
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);

  const { yMin, yMax } = useMemo(() => {
    let min = forcedYMin ?? Infinity;
    let max = forcedYMax ?? -Infinity;
    if (forcedYMin == null || forcedYMax == null) {
      for (const s of series) {
        for (const v of s.values) {
          if (!isFinite(v)) continue;
          if (forcedYMin == null && v < min) min = v;
          if (forcedYMax == null && v > max) max = v;
        }
      }
    }
    if (!isFinite(min)) min = 0;
    if (!isFinite(max)) max = 10;
    if (max - min < 2) { min -= 1; max += 1; }
    const pad = (max - min) * 0.1;
    if (forcedYMin == null) min -= pad;
    if (forcedYMax == null) max += pad;
    return { yMin: min, yMax: max };
  }, [series, forcedYMin, forcedYMax]);

  const tMin = timestamps.length > 0 ? timestamps[0] : 0;
  const tMax = timestamps.length > 1 ? timestamps[timestamps.length - 1] : tMin + 1;
  const tRange = tMax - tMin || 1;

  const toX = useCallback((t: number) => PAD.left + ((t - tMin) / tRange) * plotW, [tMin, tRange, plotW]);
  const toY = useCallback((v: number) => PAD.top + plotH - ((v - yMin) / (yMax - yMin)) * plotH, [yMin, yMax, plotH]);

  const paths = useMemo(() => {
    return series.map((s) => {
      const pts: string[] = [];
      for (let i = 0; i < s.values.length && i < timestamps.length; i++) {
        const v = s.values[i];
        if (!isFinite(v)) continue;
        pts.push(`${pts.length === 0 ? "M" : "L"}${toX(timestamps[i]).toFixed(1)},${toY(v).toFixed(1)}`);
      }
      return { ...s, d: pts.join("") };
    });
  }, [series, timestamps, toX, toY]);

  const yTicks = useMemo(() => {
    const ticks: number[] = [];
    const step = niceStep(yMin, yMax, 6);
    const start = Math.ceil(yMin / step) * step;
    for (let v = start; v <= yMax + step * 0.01; v += step) {
      ticks.push(Math.round(v * 100) / 100);
    }
    return ticks;
  }, [yMin, yMax]);

  const xTicks = useMemo(() => {
    const ticks: { t: number; label: string }[] = [];
    const count = Math.min(10, Math.max(3, Math.floor(plotW / 60)));
    for (let i = 0; i < count; i++) {
      const t = tMin + (tRange * i) / (count - 1);
      const sec = Math.round((t - tMax) / 1000);
      ticks.push({ t, label: sec === 0 ? "now" : `${sec}s` });
    }
    return ticks;
  }, [tMin, tMax, tRange, plotW]);

  // Find nearest data index from mouse X position
  const handleMouseMove = useCallback(
    (e: React.MouseEvent<SVGSVGElement>) => {
      const svg = e.currentTarget;
      const rect = svg.getBoundingClientRect();
      const mouseX = ((e.clientX - rect.left) / rect.width) * width;
      const plotX = mouseX - PAD.left;
      if (plotX < 0 || plotX > plotW) { setHoverIdx(null); return; }
      const t = tMin + (plotX / plotW) * tRange;
      // Binary-ish search for nearest timestamp
      let best = 0;
      let bestDist = Infinity;
      for (let i = 0; i < timestamps.length; i++) {
        const dist = Math.abs(timestamps[i] - t);
        if (dist < bestDist) { bestDist = dist; best = i; }
      }
      setHoverIdx(best);
    },
    [width, plotW, tMin, tRange, timestamps],
  );

  const handleMouseLeave = useCallback(() => setHoverIdx(null), []);

  if (timestamps.length < 2) {
    return (
      <div className="flex items-center justify-center text-gray-600 text-xs italic" style={{ height }}>
        Waiting for data...
      </div>
    );
  }

  // Values at hover or latest
  const displayIdx = hoverIdx ?? timestamps.length - 1;
  const displayValues = series.map((s) => {
    const v = displayIdx < s.values.length ? s.values[displayIdx] : NaN;
    return isFinite(v) ? v : NaN;
  });
  const displayTime = timestamps[displayIdx] ?? tMax;
  const displayRelSec = Math.round((displayTime - tMax) / 1000);

  return (
    <div>
      <svg
        viewBox={`0 0 ${width} ${height}`}
        width="100%"
        className="select-none cursor-crosshair"
        onMouseMove={handleMouseMove}
        onMouseLeave={handleMouseLeave}
      >
        <defs>
          <filter id={`glow-${uid}`} x="-20%" y="-20%" width="140%" height="140%">
            <feGaussianBlur stdDeviation="1.5" result="blur" />
            <feMerge>
              <feMergeNode in="blur" />
              <feMergeNode in="SourceGraphic" />
            </feMerge>
          </filter>
          <clipPath id={`clip-${uid}`}>
            <rect x={PAD.left} y={PAD.top} width={plotW} height={plotH} />
          </clipPath>
          {/* Gradient fill under measured line */}
          {series.map((s) => (
            <linearGradient key={s.key} id={`grad-${uid}-${s.key}`} x1="0" x2="0" y1="0" y2="1">
              <stop offset="0%" stopColor={s.color} stopOpacity={0.15} />
              <stop offset="100%" stopColor={s.color} stopOpacity={0.0} />
            </linearGradient>
          ))}
        </defs>

        {/* Plot background */}
        <rect x={PAD.left} y={PAD.top} width={plotW} height={plotH} fill="#060a14" rx={4} />
        <rect x={PAD.left} y={PAD.top} width={plotW} height={plotH} fill="none" stroke="#1a2332" strokeWidth={1} rx={4} />

        {/* Y grid */}
        {yTicks.map((v) => (
          <g key={v}>
            <line x1={PAD.left} x2={PAD.left + plotW} y1={toY(v)} y2={toY(v)} stroke="#141e2e" strokeWidth={1} />
            <text x={PAD.left - 8} y={toY(v) + 3.5} fill="#3b4a5c" fontSize={10} textAnchor="end" fontFamily="ui-monospace, monospace">
              {v.toFixed(1)}
            </text>
          </g>
        ))}

        {/* Y-axis label */}
        {yLabel && (
          <text x={10} y={PAD.top + plotH / 2} fill="#3b4a5c" fontSize={10} textAnchor="middle" transform={`rotate(-90, 10, ${PAD.top + plotH / 2})`}>
            {yLabel}
          </text>
        )}

        {/* X-axis labels */}
        {xTicks.map(({ t, label }) => (
          <g key={t}>
            <line x1={toX(t)} x2={toX(t)} y1={PAD.top + plotH} y2={PAD.top + plotH + 4} stroke="#1a2332" strokeWidth={1} />
            <text x={toX(t)} y={height - 10} fill="#3b4a5c" fontSize={10} textAnchor="middle" fontFamily="ui-monospace, monospace">
              {label}
            </text>
          </g>
        ))}

        {/* Area fill under first non-dashed series */}
        <g clipPath={`url(#clip-${uid})`}>
          {paths.map((p) => {
            if (p.dashed || !p.d) return null;
            const closedD = `${p.d}L${toX(tMax).toFixed(1)},${(PAD.top + plotH).toFixed(1)}L${toX(tMin).toFixed(1)},${(PAD.top + plotH).toFixed(1)}Z`;
            return <path key={`fill-${p.key}`} d={closedD} fill={`url(#grad-${uid}-${p.key})`} />;
          })}
        </g>

        {/* Data lines */}
        <g clipPath={`url(#clip-${uid})`} filter={`url(#glow-${uid})`}>
          {paths.map((p) =>
            p.d ? (
              <path
                key={p.key}
                d={p.d}
                fill="none"
                stroke={p.color}
                strokeWidth={p.dashed ? 1.5 : 2}
                strokeDasharray={p.dashed ? "6,4" : undefined}
                strokeLinejoin="round"
                strokeLinecap="round"
              />
            ) : null,
          )}
        </g>

        {/* Hover crosshair + dots */}
        {hoverIdx != null && (
          <g clipPath={`url(#clip-${uid})`}>
            {/* Vertical line */}
            <line
              x1={toX(timestamps[hoverIdx])}
              x2={toX(timestamps[hoverIdx])}
              y1={PAD.top}
              y2={PAD.top + plotH}
              stroke="#475569"
              strokeWidth={1}
              strokeDasharray="3,3"
            />
            {/* Dots on each series */}
            {series.map((s, i) => {
              const v = hoverIdx < s.values.length ? s.values[hoverIdx] : NaN;
              if (!isFinite(v)) return null;
              return (
                <circle
                  key={s.key}
                  cx={toX(timestamps[hoverIdx])}
                  cy={toY(v)}
                  r={4}
                  fill={s.color}
                  stroke="#060a14"
                  strokeWidth={2}
                />
              );
            })}
          </g>
        )}

        {/* Latest-value dots when not hovering */}
        {hoverIdx == null && (
          <g clipPath={`url(#clip-${uid})`}>
            {series.map((s) => {
              const lastI = Math.min(s.values.length - 1, timestamps.length - 1);
              if (lastI < 0) return null;
              const v = s.values[lastI];
              if (!isFinite(v)) return null;
              return (
                <circle key={s.key} cx={toX(timestamps[lastI])} cy={toY(v)} r={3} fill={s.color} stroke="#060a14" strokeWidth={1.5} />
              );
            })}
          </g>
        )}

        {/* Title */}
        {title && (
          <text x={PAD.left + 8} y={PAD.top + 14} fill="#6b7280" fontSize={11} fontWeight={500}>
            {title}
          </text>
        )}
      </svg>

      {/* Legend with values at hover point or latest */}
      <div className="flex flex-wrap items-center justify-center gap-x-5 gap-y-1 mt-1.5 px-2">
        <span className="text-[10px] font-mono text-gray-600">
          {hoverIdx != null ? (displayRelSec === 0 ? "now" : `${displayRelSec}s`) : "latest"}
        </span>
        {series.map((s, i) => (
          <div key={s.key} className="flex items-center gap-1.5 text-[11px]">
            <span className="inline-block w-3 h-0.5 rounded-full shrink-0" style={{ backgroundColor: s.color, opacity: s.dashed ? 0.6 : 1 }} />
            <span className="text-gray-500">{s.label}</span>
            <span className="font-mono font-medium" style={{ color: s.color }}>
              {isFinite(displayValues[i]) ? displayValues[i].toFixed(1) : "—"}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

function niceStep(min: number, max: number, targetTicks: number): number {
  const rawStep = (max - min) / targetTicks;
  if (rawStep <= 0) return 1;
  const mag = Math.pow(10, Math.floor(Math.log10(rawStep)));
  const norm = rawStep / mag;
  let nice: number;
  if (norm <= 1) nice = 1;
  else if (norm <= 2) nice = 2;
  else if (norm <= 5) nice = 5;
  else nice = 10;
  return nice * mag;
}
