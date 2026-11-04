// Phase 5: `this.reportProgress` is inherited across two modules
// (`ExportProcessor` → `AccountingProcessor` → `BaseProcessor`); no analyzed
// subclass overrides it, so the call dispatches to `BaseProcessor` and the
// publication after the commit is traced.
// @expect MW1001 line=12 confidence=high function=ExportProcessor.process
import { db } from "@/db";
import { AccountingProcessor } from "./accounting";

export class ExportProcessor extends AccountingProcessor {
  async process(id: string) {
    await db.update(exports).set({ status: "done" });
    await this.reportProgress(id, 100);
  }
}
