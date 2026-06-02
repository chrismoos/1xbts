"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

const links = [
  { href: "/", label: "Dashboard" },
  { href: "/radio", label: "Radio" },
  { href: "/messages", label: "Messages" },
  { href: "/channels", label: "Channels" },
  { href: "/mobiles", label: "Mobiles" },
  { href: "/subscribers", label: "Subscribers" },
  { href: "/prls", label: "PRLs" },
  { href: "/smsc", label: "SMSC" },
  { href: "/packets", label: "Packets" },
  { href: "/config", label: "Config" },
];

export function Nav() {
  const pathname = usePathname();

  return (
    <nav className="border-b border-gray-800 bg-gray-900/80 backdrop-blur-sm px-6 py-3 sticky top-0 z-50">
      <div className="flex items-center gap-8">
        <span className="text-sm font-bold text-green-400 tracking-wider">1xBTS</span>
        <div className="flex gap-1">
          {links.map((link) => {
            const active =
              link.href === "/"
                ? pathname === "/"
                : pathname.startsWith(link.href);
            return (
              <Link
                key={link.href}
                href={link.href}
                className={`text-sm px-3 py-1 rounded transition-colors ${
                  active
                    ? "bg-gray-800 text-gray-100"
                    : "text-gray-500 hover:text-gray-300 hover:bg-gray-800/50"
                }`}
              >
                {link.label}
              </Link>
            );
          })}
        </div>
      </div>
    </nav>
  );
}
