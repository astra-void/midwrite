// Phase 5: `Worker` extends a class from a package. `this.finish` is found
// in `Worker` itself, but an unanalyzed superclass constructor could install
// an instance property `finish`: the dispatch is resolved under that
// recorded assumption, and confidence is capped at medium.
// @expect MW1001 line=12 confidence=medium function=Worker.run
import { db } from "@/db";
import { WorkerHost } from "@nestjs/bullmq";

export class Worker extends WorkerHost {
  async run(id: string) {
    await db.update(jobs).set({ status: "done" });
    await this.finish(id);
  }

  private async finish(id: string) {
    await queue.publish("job.finished", { id });
  }
}
