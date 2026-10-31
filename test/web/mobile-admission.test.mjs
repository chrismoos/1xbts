import assert from "node:assert/strict";
import test from "node:test";
import { latestAdmissionForMobile } from "../../1xbts-web/src/lib/admission.ts";

const older = "2026-10-30T02:41:00Z";
const newer = "2026-10-30T02:45:00Z";

test("a deleted subscriber's denial replaces its earlier admission", () => {
  const mobile = { imsi: "310006026392922", esn: 0x36262f13 };
  const admitted = { ...mobile, time: older, admissionClass: "admitted" };
  const denied = { ...mobile, time: newer, admissionClass: "denied" };
  assert.equal(latestAdmissionForMobile([admitted, denied], mobile), denied);
});

test("hardware follows the newest decision across an OTASP IMSI change", () => {
  const staged = { imsi: "310009176936269", esn: 0x4cdc1d09, time: older, admissionClass: "provisioning" };
  const admitted = { ...staged, imsi: "310550000001951", time: newer, admissionClass: "admitted" };
  assert.equal(latestAdmissionForMobile([staged, admitted], admitted), admitted);
});

test("pseudo-ESN collisions do not transfer admission to another device", () => {
  const first = { imsi: "310550000001951", esn: 0x80123456, meid: "A0000012345678" };
  const second = { ...first, meid: "A0000087654321", admissionClass: "admitted" };
  assert.equal(latestAdmissionForMobile([second], first), undefined);
  assert.equal(latestAdmissionForMobile([{ ...second, meid: undefined }], { ...first, imsi: "310550000001004" }), undefined);
});

test("MEID matching ignores case and missing snapshots stay unknown", () => {
  const event = { meid: "A0000012345678", admissionClass: "denied" };
  assert.equal(latestAdmissionForMobile([event], { meid: "a0000012345678" }), event);
  assert.equal(latestAdmissionForMobile([], { imsi: "310550000001951" }), undefined);
  assert.equal(latestAdmissionForMobile([event], {}), undefined);
});
