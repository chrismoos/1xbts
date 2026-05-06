"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { useSSEConnected } from "@/lib/use-event-stream";

const links = [
  { href: "/", label: "Dashboard" },
  { href: "/radio", label: "Radio" },
  { href: "/messages", label: "Messages" },
  { href: "/channels", label: "Channels" },
  { href: "/mobiles", label: "Mobiles" },
  { href: "/subscribers", label: "Subscribers" },
  { href: "/smsc", label: "SMSC" },
  { href: "/packets", label: "Packets" },
  { href: "/config", label: "Config" },
];

type Theme = "dark" | "light";

const THEME_CHANGE_EVENT = "cdma-web-theme-change";

function getThemeSnapshot(): Theme {
  if (typeof window === "undefined") {
    return "dark";
  }
  return localStorage.getItem("theme") === "light" ? "light" : "dark";
}

function subscribeThemeChange(onStoreChange: () => void) {
  window.addEventListener("storage", onStoreChange);
  window.addEventListener(THEME_CHANGE_EVENT, onStoreChange);
  return () => {
    window.removeEventListener("storage", onStoreChange);
    window.removeEventListener(THEME_CHANGE_EVENT, onStoreChange);
  };
}

function useSidebarTheme() {
  return useSyncExternalStore(subscribeThemeChange, getThemeSnapshot, () => "dark");
}

function ThemeToggle() {
  const theme = useSidebarTheme();

  useEffect(() => {
    document.documentElement.classList.toggle("light", theme === "light");
  }, [theme]);

  const toggle = () => {
    const next = theme === "dark" ? "light" : "dark";
    localStorage.setItem("theme", next);
    document.documentElement.classList.toggle("light", next === "light");
    window.dispatchEvent(new Event(THEME_CHANGE_EVENT));
  };

  return (
    <button
      onClick={toggle}
      className="flex items-center gap-2 w-full px-3 py-2 rounded-lg text-[13px] font-medium text-muted hover:text-primary hover:bg-hover transition-colors"
      title={`Switch to ${theme === "dark" ? "light" : "dark"} mode`}
    >
      {theme === "dark" ? (
        <svg className="w-4 h-4" viewBox="0 0 20 20" fill="currentColor">
          <path fillRule="evenodd" d="M10 2a1 1 0 011 1v1a1 1 0 11-2 0V3a1 1 0 011-1zm4 8a4 4 0 11-8 0 4 4 0 018 0zm-.464 4.95l.707.707a1 1 0 001.414-1.414l-.707-.707a1 1 0 00-1.414 1.414zm2.12-10.607a1 1 0 010 1.414l-.706.707a1 1 0 11-1.414-1.414l.707-.707a1 1 0 011.414 0zM17 11a1 1 0 100-2h-1a1 1 0 100 2h1zm-7 4a1 1 0 011 1v1a1 1 0 11-2 0v-1a1 1 0 011-1zM5.05 6.464A1 1 0 106.465 5.05l-.708-.707a1 1 0 00-1.414 1.414l.707.707zm1.414 8.486l-.707.707a1 1 0 01-1.414-1.414l.707-.707a1 1 0 011.414 1.414zM4 11a1 1 0 100-2H3a1 1 0 000 2h1z" clipRule="evenodd" />
        </svg>
      ) : (
        <svg className="w-4 h-4" viewBox="0 0 20 20" fill="currentColor">
          <path d="M17.293 13.293A8 8 0 016.707 2.707a8.001 8.001 0 1010.586 10.586z" />
        </svg>
      )}
      {theme === "dark" ? "Light mode" : "Dark mode"}
    </button>
  );
}

interface BscStatus {
  pnOffset?: number;
  bandClass?: number;
  cdmaChannel?: number;
  sid?: number;
  nid?: number;
}

function useBscStatus() {
  const [status, setStatus] = useState<BscStatus | null>(null);
  const connected = useSSEConnected();

  const fetchStatus = useCallback(() => {
    fetch("/api/system-status")
      .then((r) => r.json())
      .then((data) => { if (!data.error) setStatus(data); })
      .catch(() => setStatus(null));
  }, []);

  useEffect(() => {
    fetchStatus();
    const interval = setInterval(fetchStatus, 10000);
    return () => clearInterval(interval);
  }, [fetchStatus]);

  return { status, connected };
}

function BrandLogo() {
  const theme = useSidebarTheme();
  return (
    <div className="px-4 pt-5 pb-4">
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img
        src={theme === "light" ? "/logo.svg" : "/logo-dark.svg"}
        alt="1xBTS"
        className="h-16 w-auto"
      />
    </div>
  );
}

export function Sidebar() {
  const pathname = usePathname();
  const { status, connected } = useBscStatus();

  return (
    <aside className="glass-sidebar flex flex-col h-full sticky top-0">
      {/* Brand */}
      <BrandLogo />


      {/* Nav links */}
      <nav className="flex-1 px-2 flex flex-col gap-0.5">
        {links.map((link) => {
          const active =
            link.href === "/"
              ? pathname === "/"
              : pathname.startsWith(link.href);
          return (
            <Link
              key={link.href}
              href={link.href}
              className={`flex items-center gap-2.5 text-[13px] font-medium px-3 py-2 rounded-lg transition-colors ${
                active
                  ? "bg-active text-primary font-semibold"
                  : "text-muted hover:text-primary hover:bg-hover"
              }`}
            >
              <span
                className={`w-1.5 h-1.5 rounded-full shrink-0 ${
                  active ? "bg-accent-indigo shadow-[0_0_6px_var(--accent-indigo)]" : "bg-dimmed"
                }`}
              />
              {link.label}
            </Link>
          );
        })}
      </nav>

      {/* Footer */}
      <div className="px-2 py-3 border-t border-border space-y-1">
        <div className="px-2">
          <div className={`flex items-center gap-1.5 text-xs font-medium ${connected ? "text-primary" : "text-accent-red"}`}>
            <span
              className={`w-1.5 h-1.5 rounded-full ${
                connected
                  ? "bg-live shadow-[0_0_6px_var(--live-color)]"
                  : "bg-accent-red"
              }`}
            />
            {connected ? "Online" : "Offline"}
          </div>
          <div className="text-[11px] text-dimmed mt-1 font-mono">
            {status
              ? `PN ${status.pnOffset ?? "-"} · BC${status.bandClass ?? "-"} CH.${status.cdmaChannel ?? "-"} · SID ${status.sid ?? "-"}`
              : "—"}
          </div>
        </div>
        <ThemeToggle />
      </div>
    </aside>
  );
}
