// @expect none
// DEV-170: a repository class in its own module. Its methods, analyzed on
// their own, do not know what `this.database` holds; each caller decides.
export class OrderRepository {
  constructor(private readonly database: any) {}
  async markPaid(id: string) {
    await this.database.update(orders).set({ status: "paid" });
  }
  async complete(id: string) {
    await this.database.update(orders).set({ status: "paid" });
    await this.database.insert(lines).values({ id });
  }
}
