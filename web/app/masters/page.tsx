"use client";
// Customers & suppliers, products (with their eTIMS fields), vehicles.
// Changes reach every station on its next sync (about 10 seconds when online).

import { useEffect, useState } from "react";
import { Crud } from "@/components/crud";
import { Shell } from "@/components/ui";
import { api, canManage, kg, money, type User } from "@/lib/api";

export default function MastersPage() {
  return <Shell>{(u) => <Masters user={u} />}</Shell>;
}

const TAX = [
  { value: "B", label: "B - VAT 16%" },
  { value: "E", label: "E - VAT 8%" },
  { value: "A", label: "A - Exempt" },
  { value: "C", label: "C - Zero rated" },
  { value: "D", label: "D - Non-VAT" },
];
const UNITS = [
  { value: "TNE", label: "TNE - tonne (net weight / 1000)" },
  { value: "KG", label: "KG - kilogram (net weight)" },
  { value: "U", label: "U - each / service" },
];

function Masters({ user }: { user: User }) {
  const [tab, setTab] = useState("parties");
  const [parties, setParties] = useState<any[]>([]);
  useEffect(() => {
    api<any[]>("/api/parties?all=true").then(setParties).catch(() => {});
  }, [tab]);
  const edit = canManage(user);
  return (
    <>
      <h1>Customers & items</h1>
      <div className="tabs">
        {[["parties", "Customers & suppliers"], ["products", "Products"], ["vehicles", "Vehicles"]].map(([k, l]) => (
          <button key={k} className={tab === k ? "active" : ""} onClick={() => setTab(k)}>{l}</button>
        ))}
      </div>
      {tab === "parties" && (
        <Crud
          title="Customer / supplier"
          endpoint="/api/parties"
          canEdit={edit}
          fields={[
            { key: "code", label: "Code" },
            { key: "name", label: "Name" },
            { key: "kind", label: "Type", type: "select", default: "customer", options: [{ value: "customer", label: "Customer" }, { value: "supplier", label: "Supplier" }, { value: "both", label: "Both" }] },
            { key: "tin", label: "KRA PIN", help: "Printed on invoices and sent to eTIMS as the buyer PIN" },
            { key: "phone", label: "Phone" },
            { key: "email", label: "Email", formOnly: true },
            { key: "address", label: "Address", formOnly: true },
            { key: "active", label: "Active", type: "bool" },
          ]}
        />
      )}
      {tab === "products" && (
        <Crud
          title="Product"
          endpoint="/api/products"
          canEdit={edit}
          fields={[
            { key: "code", label: "Code", help: "Use FEE for the weighing-fee item" },
            { key: "name", label: "Name" },
            { key: "unit_price", label: "Price", type: "number", render: (r) => money(r.unit_price) },
            { key: "quantity_unit", label: "Sold per", type: "select", default: "TNE", options: UNITS },
            { key: "tax_type_code", label: "Tax", type: "select", default: "B", options: TAX },
            { key: "item_code", label: "eTIMS item code", help: "If the item is registered in eTIMS. Otherwise the classification + name are sent." },
            { key: "classification_code", label: "KRA classification", help: "KRA item classification code (e.g. 1111150000)" },
            { key: "packaging_unit", label: "Packaging unit", formOnly: true, default: "NT" },
            { key: "active", label: "Active", type: "bool" },
          ]}
        />
      )}
      {tab === "vehicles" && (
        <Crud
          title="Vehicle"
          endpoint="/api/vehicles"
          canEdit={edit}
          fields={[
            { key: "plate", label: "Number plate" },
            { key: "stored_tare_kg", label: "Stored tare", type: "number", render: (r) => kg(r.stored_tare_kg), help: "Empty weight. Filled in automatically after each completed weighing; used for single-pass weighing." },
            { key: "party_id", label: "Owner", type: "select", options: [{ value: "", label: "-" }, ...parties.map((p) => ({ value: p.id, label: p.name }))] },
            { key: "driver_name", label: "Usual driver" },
            { key: "active", label: "Active", type: "bool" },
          ]}
        />
      )}
    </>
  );
}
