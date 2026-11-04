"use client";

import { useLive } from "@/lib/live";

import { Card } from "@/components/card";
import { useBtsList } from "@/lib/use-bts-list";
import { useAdmissionVerdicts } from "@/components/access-activity";
import { AccessDeniedCard } from "@/components/access-denied-card";
import { MobilesTable, type MobileInfo } from "@/components/mobiles-table";
import { ErrorNotice } from "@/components/error-notice";

const NO_MOBILES: MobileInfo[] = [];

export default function MobilesPage() {
  const live = useLive<MobileInfo[]>("/api/mobiles");
  const mobiles = Array.isArray(live.data) ? live.data : NO_MOBILES;
  const { loading } = live;
  const error = live.errorBody ?? live.error;
  const { cells } = useBtsList();
  const { policy } = useAdmissionVerdicts({}, 1);
  const closed = policy?.accessMode === "closed";


  return (
    <div className="page-container space-y-6">
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-bold">Mobile Stations</h1>
        {policy && (
          <span
            className={`text-xs px-2 py-0.5 rounded ${
              closed ? "bg-badge-red-bg text-badge-red-text" : "bg-badge-green-bg text-badge-green-text"
            }`}
          >
            Access: {closed ? "Closed" : "Open"}
          </span>
        )}
      </div>

      <Card title={`Registered Mobiles (${mobiles.length})`}>
        {loading ? (
          <p className="text-dimmed text-sm">Loading...</p>
        ) : error ? (
          <ErrorNotice error={error} />
        ) : mobiles.length === 0 ? (
          <p className="text-dimmed text-sm">
            No mobile stations registered.
          </p>
        ) : (
          <MobilesTable mobiles={mobiles} cells={cells} closed={closed} />
        )}
      </Card>

      {closed && <AccessDeniedCard />}
    </div>
  );
}
