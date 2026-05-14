"use strict";
/**
 * Built-in CostSink implementations: in-memory, local-file, Supabase.
 */
Object.defineProperty(exports, "__esModule", { value: true });
exports.SupabaseSink = exports.FileSink = exports.MemorySink = void 0;
const promises_1 = require("node:fs/promises");
const node_path_1 = require("node:path");
class MemorySink {
    records = [];
    async write(records) {
        for (const r of records)
            this.records.push(r);
    }
    async read() { return this.records.slice(); }
}
exports.MemorySink = MemorySink;
class FileSink {
    path;
    constructor(path) {
        this.path = path;
    }
    async write(records) {
        await (0, promises_1.mkdir)((0, node_path_1.dirname)(this.path), { recursive: true });
        let existing = [];
        try {
            existing = JSON.parse(await (0, promises_1.readFile)(this.path, 'utf8'));
        }
        catch { /* new file */ }
        existing.push(...records);
        await (0, promises_1.writeFile)(this.path, JSON.stringify(existing, null, 2));
    }
    async read() {
        try {
            return JSON.parse(await (0, promises_1.readFile)(this.path, 'utf8'));
        }
        catch {
            return [];
        }
    }
}
exports.FileSink = FileSink;
/** Supabase REST sink. Writes via PostgREST so we don't need the JS client
 *  as a dependency — keeps the package zero-runtime-dep. */
class SupabaseSink {
    url;
    key;
    table;
    budgetTable;
    viewName;
    constructor(opts) {
        this.url = opts.url.replace(/\/$/, '');
        this.key = opts.key;
        this.table = opts.table ?? 'cost_records';
        this.budgetTable = opts.budgetTable ?? 'cost_budgets';
        this.viewName = opts.viewName ?? 'cost_budget_status';
    }
    headers(prefer = 'return=minimal') {
        return {
            apikey: this.key,
            Authorization: `Bearer ${this.key}`,
            'Content-Type': 'application/json',
            Prefer: prefer,
        };
    }
    async write(records) {
        if (records.length === 0)
            return;
        const res = await fetch(`${this.url}/rest/v1/${this.table}`, {
            method: 'POST',
            headers: this.headers(),
            body: JSON.stringify(records),
        });
        if (!res.ok) {
            const body = await res.text().catch(() => '');
            throw new Error(`SupabaseSink write failed: ${res.status} ${body.slice(0, 200)}`);
        }
    }
    async read(agent_id, since) {
        const url = `${this.url}/rest/v1/${this.table}?agent_id=eq.${encodeURIComponent(agent_id)}&created_at=gte.${since.toISOString()}&select=*`;
        const res = await fetch(url, { headers: this.headers('return=representation') });
        if (!res.ok)
            throw new Error(`SupabaseSink read failed: ${res.status}`);
        return await res.json();
    }
    async readBudgets(agent_id) {
        const url = `${this.url}/rest/v1/${this.viewName}?agent_id=eq.${encodeURIComponent(agent_id)}&select=*`;
        const res = await fetch(url, { headers: this.headers('return=representation') });
        if (!res.ok)
            throw new Error(`SupabaseSink readBudgets failed: ${res.status}`);
        const rows = await res.json();
        return rows.map(r => ({
            category: r.category,
            allocated_usd: Number(r.allocated_usd),
            spent_usd: Number(r.spent_usd),
            remaining_usd: Number(r.remaining_usd),
            utilization_pct: Number(r.utilization_pct),
            is_over_budget: !!r.is_over_budget,
            period: r.period,
            starts_at: r.starts_at,
        }));
    }
    async writeBudget(agent_id, category, allocated_usd, period, starts_at) {
        // Upsert by (agent_id, category, period, starts_at).
        const url = `${this.url}/rest/v1/${this.budgetTable}?on_conflict=agent_id,category,period,starts_at`;
        const res = await fetch(url, {
            method: 'POST',
            headers: { ...this.headers('resolution=merge-duplicates,return=minimal') },
            body: JSON.stringify({ agent_id, category, allocated_usd, period, starts_at: starts_at.toISOString() }),
        });
        if (!res.ok) {
            const body = await res.text().catch(() => '');
            throw new Error(`SupabaseSink writeBudget failed: ${res.status} ${body.slice(0, 200)}`);
        }
    }
}
exports.SupabaseSink = SupabaseSink;
//# sourceMappingURL=sinks.js.map