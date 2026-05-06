"use client";

import { useEffect, useState, useCallback } from "react";
import Link from "next/link";
import { Card } from "@/components/card";

interface Subscriber {
  subscriberId: string;
  phoneNumber: string;
  displayName: string;
  status: string;
  createdAt?: { seconds: number; nanos: number };
  updatedAt?: { seconds: number; nanos: number };
}

interface SubscriberIdentity {
  subscriberIdentityId: string;
  subscriberId: string;
  imsi?: string;
  esn?: number;
  isPrimary: boolean;
}

interface UpsertResponse {
  subscriber?: Subscriber;
  identity?: SubscriberIdentity;
  error?: string;
}

interface MobileInfo {
  address: string;
  state: string;
  phoneNumber?: string;
  subscriberId?: string;
}

function formatSubscriberId(subscriberId: string): string {
  if (subscriberId.length <= 24) return subscriberId;
  return `${subscriberId.slice(0, 12)}...${subscriberId.slice(-8)}`;
}

export default function SubscribersPage() {
  const [subscribers, setSubscribers] = useState<Subscriber[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showForm, setShowForm] = useState(false);
  const [mobiles, setMobiles] = useState<MobileInfo[]>([]);

  // Form state
  const [phoneNumber, setPhoneNumber] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [esnHex, setEsnHex] = useState("");
  const [imsi, setImsi] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const fetchSubscribers = useCallback(async () => {
    try {
      const res = await fetch("/api/subscribers");
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      if (data.error) throw new Error(data.error);
      setSubscribers(data.subscribers || []);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "unknown error");
    } finally {
      setLoading(false);
    }
  }, []);

  const fetchMobiles = useCallback(async () => {
    try {
      const res = await fetch("/api/mobiles");
      if (res.ok) {
        const data = await res.json();
        if (!data.error) setMobiles(data);
      }
    } catch {}
  }, []);

  useEffect(() => {
    fetchSubscribers();
    fetchMobiles();
    const interval = setInterval(() => { fetchSubscribers(); fetchMobiles(); }, 5000);
    return () => clearInterval(interval);
  }, [fetchSubscribers, fetchMobiles]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setFormError(null);
    setSubmitting(true);

    try {
      const body: Record<string, unknown> = {
        phoneNumber: phoneNumber.trim(),
        displayName,
        status: "active",
      };

      const normalizedPhoneNumber = phoneNumber.trim();
      const normalizedEsn = esnHex.trim();
      const normalizedImsi = imsi.trim();

      if (!/^\d+$/.test(normalizedPhoneNumber)) {
        setFormError("Phone number must contain at least one digit and only digits");
        return;
      }

      if (!normalizedEsn && !normalizedImsi) {
        setFormError("Enter ESN, IMSI, or both");
        return;
      }

      if (normalizedEsn) {
        const esn = parseInt(esnHex, 16);
        if (isNaN(esn)) {
          setFormError("Invalid ESN hex value");
          return;
        }
        body.esn = esn;
      }

      if (normalizedImsi) {
        if (!/^\d{10,15}$/.test(normalizedImsi)) {
          setFormError("IMSI must be 10 to 15 digits");
          return;
        }
        body.imsi = normalizedImsi;
      }

      const res = await fetch("/api/subscribers", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      const data: UpsertResponse = await res.json();
      if (data.error) throw new Error(data.error);

      if (data.subscriber) {
        const created = {
          ...data.subscriber,
          displayName: data.subscriber.displayName || displayName.trim(),
        };
        setSubscribers((prev) => [
          created,
          ...prev.filter((sub) => sub.subscriberId !== created.subscriberId),
        ]);
      }

      // Reset form
      setPhoneNumber("");
      setDisplayName("");
      setEsnHex("");
      setImsi("");
      setShowForm(false);
      void fetchSubscribers();
    } catch (err) {
      setFormError(err instanceof Error ? err.message : "unknown error");
    } finally {
      setSubmitting(false);
    }
  };

  const handleDelete = async (subscriberId: string) => {
    try {
      const res = await fetch(`/api/subscribers?id=${encodeURIComponent(subscriberId)}`, {
        method: "DELETE",
      });
      const data = await res.json();
      if (data.error) throw new Error(data.error);
      fetchSubscribers();
    } catch (err) {
      setError(err instanceof Error ? err.message : "unknown error");
    }
  };

  return (
    <div className="max-w-7xl mx-auto space-y-6">
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-bold">Subscribers</h1>
        <button
          onClick={() => setShowForm(!showForm)}
          className="text-xs px-3 py-1.5 rounded bg-accent-green-bg text-accent-green border border-accent-green/20 hover:bg-accent-green/15 transition-colors"
        >
          {showForm ? "Cancel" : "Add Subscriber"}
        </button>
      </div>

      {showForm && (
        <Card title="New Subscriber">
          <form onSubmit={handleSubmit} className="space-y-4">
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-xs text-muted mb-1">Phone Number</label>
                <input
                  type="text"
                  value={phoneNumber}
                  onChange={(e) => setPhoneNumber(e.target.value)}
                  placeholder="5551234567"
                  required
                  className="w-full glass-input font-mono"
                />
              </div>
              <div>
                <label className="block text-xs text-muted mb-1">Display Name</label>
                <input
                  type="text"
                  value={displayName}
                  onChange={(e) => setDisplayName(e.target.value)}
                  placeholder="Test Phone 1"
                  className="w-full glass-input"
                />
              </div>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div>
                <label className="block text-xs text-muted mb-1">ESN (hex)</label>
                <input
                  type="text"
                  value={esnHex}
                  onChange={(e) => setEsnHex(e.target.value)}
                  placeholder="A0000001"
                  maxLength={8}
                  className="w-full glass-input font-mono"
                />
              </div>
              <div>
                <label className="block text-xs text-muted mb-1">IMSI</label>
                <input
                  type="text"
                  value={imsi}
                  onChange={(e) => setImsi(e.target.value)}
                  placeholder="12345678901"
                  className="w-full glass-input font-mono"
                />
              </div>
            </div>

            {formError && (
              <p className="text-accent-red text-xs">{formError}</p>
            )}

            <button
              type="submit"
              disabled={submitting}
              className="text-xs px-4 py-1.5 rounded bg-accent-green-bg text-accent-green border border-accent-green/20 hover:bg-accent-green/15 disabled:opacity-50 transition-colors"
            >
              {submitting ? "Creating..." : "Create Subscriber"}
            </button>
          </form>
        </Card>
      )}

      <Card title={`Provisioned Subscribers (${subscribers.length})`}>
        {loading ? (
          <p className="text-dimmed text-sm">Loading...</p>
        ) : error ? (
          <p className="text-accent-red text-sm">{error}</p>
        ) : subscribers.length === 0 ? (
          <p className="text-dimmed text-sm">
            No subscribers provisioned. Click &quot;Add Subscriber&quot; to create one.
          </p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="text-muted text-xs">
                  <th className="text-left py-1">Phone Number</th>
                  <th className="text-left py-1">Display Name</th>
                  <th className="text-left py-1">Status</th>
                  <th className="text-left py-1">Active Mobile</th>
                  <th className="text-left py-1">Subscriber ID</th>
                  <th className="text-left py-1"></th>
                </tr>
              </thead>
              <tbody>
                {subscribers.map((sub) => (
                  <tr key={sub.subscriberId} className="border-t border-border hover:bg-hover">
                    <td className="py-2 text-secondary font-mono text-xs">
                      <Link
                        href={`/subscribers/${encodeURIComponent(sub.subscriberId)}`}
                        className="hover:text-accent-green transition-colors"
                      >
                        {sub.phoneNumber}
                      </Link>
                    </td>
                    <td className="py-2 text-secondary text-xs">
                      {sub.displayName || "-"}
                    </td>
                    <td className="py-2">
                      <span
                        className={`text-xs px-2 py-0.5 rounded ${
                          sub.status === "active"
                            ? "bg-badge-green-bg text-badge-green-text"
                            : sub.status === "suspended"
                              ? "bg-badge-yellow-bg text-badge-yellow-text"
                              : "bg-badge-red-bg text-badge-red-text"
                        }`}
                      >
                        {sub.status}
                      </span>
                    </td>
                    <td className="py-2 text-xs">
                      {(() => {
                        const ms = mobiles.find((m) => m.subscriberId === sub.subscriberId);
                        if (!ms) return <span className="text-dimmed">-</span>;
                        return (
                          <Link
                            href={`/mobiles/${encodeURIComponent(ms.address)}`}
                            className="text-accent-green hover:text-accent-green transition-colors"
                          >
                            {ms.state}
                          </Link>
                        );
                      })()}
                    </td>
                    <td className="py-2 text-muted font-mono text-[11px]">
                      <Link
                        href={`/subscribers/${encodeURIComponent(sub.subscriberId)}`}
                        className="hover:text-accent-green transition-colors"
                        title={sub.subscriberId}
                      >
                        {formatSubscriberId(sub.subscriberId)}
                      </Link>
                    </td>
                    <td className="py-2 text-right space-x-3">
                      <Link
                        href={`/subscribers/${encodeURIComponent(sub.subscriberId)}`}
                        className="text-xs text-accent-green hover:text-accent-green transition-colors"
                      >
                        View
                      </Link>
                      <button
                        onClick={() => handleDelete(sub.subscriberId)}
                        className="text-xs text-accent-red hover:text-accent-red transition-colors"
                      >
                        Delete
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </div>
  );
}
