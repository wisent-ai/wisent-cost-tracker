/**
 * Built-in CostSink implementations: in-memory, local-file, Supabase.
 */

import { mkdir, writeFile, readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import type { CostRecord, CostSink, BudgetStatus, BudgetPeriod } from './types.js';

export class MemorySink implements CostSink {
  records: CostRecord[] = [];
  async write(records: CostRecord[]): Promise<void> {
    for (const r of records) this.records.push(r);
  }
  async read(): Promise<CostRecord[]> { return this.records.slice(); }
}

export class FileSink implements CostSink {
  constructor(private path: string) {}
  async write(records: CostRecord[]): Promise<void> {
    await mkdir(dirname(this.path), { recursive: true });
    let existing: CostRecord[] = [];
    try { existing = JSON.parse(await readFile(this.path, 'utf8')); } catch { /* new file */ }
    existing.push(...records);
    await writeFile(this.path, JSON.stringify(existing, null, 2));
  }
  async read(): Promise<CostRecord[]> {
    try { return JSON.parse(await readFile(this.path, 'utf8')); } catch { return []; }
  }
}

export interface SupabaseSinkOptions {
  url: string;
  key: string;          // service-role key
  table?: string;       // default 'cost_records'
  budgetTable?: string; // default 'cost_budgets'
  viewName?: string;    // default 'cost_budget_status'
}

/** Supabase REST sink. Writes via PostgREST so we don't need the JS client
 *  as a dependency — keeps the package zero-runtime-dep. */
export class SupabaseSink implements CostSink {
  private url: string;
  private key: string;
  private table: string;
  private budgetTable: string;
  private viewName: string;
  constructor(opts: SupabaseSinkOptions) {
    this.url = opts.url.replace(/\/$/, '');
    this.key = opts.key;
    this.table = opts.table ?? 'cost_records';
    this.budgetTable = opts.budgetTable ?? 'cost_budgets';
    this.viewName = opts.viewName ?? 'cost_budget_status';
  }

  private headers(prefer = 'return=minimal'): Record<string, string> {
    return {
      apikey: this.key,
      Authorization: `Bearer ${this.key}`,
      'Content-Type': 'application/json',
      Prefer: prefer,
    };
  }

  async write(records: CostRecord[]): Promise<void> {
    if (records.length === 0) return;
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

  async read(agent_id: string, since: Date): Promise<CostRecord[]> {
    const url = `${this.url}/rest/v1/${this.table}?agent_id=eq.${encodeURIComponent(agent_id)}&created_at=gte.${since.toISOString()}&select=*`;
    const res = await fetch(url, { headers: this.headers('return=representation') });
    if (!res.ok) throw new Error(`SupabaseSink read failed: ${res.status}`);
    return await res.json() as CostRecord[];
  }

  async readBudgets(agent_id: string): Promise<BudgetStatus[]> {
    const url = `${this.url}/rest/v1/${this.viewName}?agent_id=eq.${encodeURIComponent(agent_id)}&select=*`;
    const res = await fetch(url, { headers: this.headers('return=representation') });
    if (!res.ok) throw new Error(`SupabaseSink readBudgets failed: ${res.status}`);
    const rows = await res.json() as Array<Record<string, any>>;
    return rows.map(r => ({
      category: r.category,
      allocated_usd: Number(r.allocated_usd),
      spent_usd: Number(r.spent_usd),
      remaining_usd: Number(r.remaining_usd),
      utilization_pct: Number(r.utilization_pct),
      is_over_budget: !!r.is_over_budget,
      period: r.period as BudgetPeriod,
      starts_at: r.starts_at,
    }));
  }

  async writeBudget(agent_id: string, category: string, allocated_usd: number, period: BudgetPeriod, starts_at: Date): Promise<void> {
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
