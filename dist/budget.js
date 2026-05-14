"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.BudgetManager = void 0;
const sinks_js_1 = require("./sinks.js");
/**
 * BudgetManager — read/write budget allocations and check spend status.
 *
 * Uses the Supabase view `cost_budget_status` (see migration) which joins
 * `cost_budgets` with the period-windowed sum of `cost_records.cost_usd`,
 * so consumers don't have to compute spent vs allocated client-side.
 */
class BudgetManager {
    sink;
    agent_id;
    constructor(opts) {
        this.agent_id = opts.agent_id;
        if (opts.sink)
            this.sink = opts.sink;
        else if (opts.supabase)
            this.sink = new sinks_js_1.SupabaseSink(opts.supabase);
        else
            throw new Error('BudgetManager: provide either opts.sink or opts.supabase');
    }
    async setBudget(category, allocated_usd, period, starts_at = new Date()) {
        if (!this.sink.writeBudget)
            throw new Error('BudgetManager: sink does not support writeBudget');
        await this.sink.writeBudget(this.agent_id, category, allocated_usd, period, starts_at);
    }
    async getStatus(category) {
        if (!this.sink.readBudgets)
            throw new Error('BudgetManager: sink does not support readBudgets');
        const all = await this.sink.readBudgets(this.agent_id);
        return category ? all.filter(b => b.category === category) : all;
    }
    async isOverBudget(category) {
        const rows = await this.getStatus(category);
        return rows.some(b => b.is_over_budget);
    }
    async remaining(category) {
        const rows = await this.getStatus(category);
        if (rows.length === 0)
            return Infinity;
        return rows.reduce((a, b) => a + b.remaining_usd, 0);
    }
}
exports.BudgetManager = BudgetManager;
//# sourceMappingURL=budget.js.map