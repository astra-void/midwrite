// @expect none
export abstract class BaseProcessor {
  protected async reportProgress(id: string, progress: number) {
    await queue.publish("job.progress", { id, progress });
  }
}
