// A method called on a field that always holds one project class is that
// class's method: a `#private` field initialized with `new Repo()`, and a
// `private readonly` field assigned once, with `new Repo()`, at the top of
// the constructor. The repository's write commits before the publication.
// A public field, or a private one assigned again elsewhere, may hold
// something else: those calls are not followed (no finding there).
// @expect MW1001 line=22 confidence=high function=PrivateField.run
// @expect MW1001 line=33 confidence=high function=CtorField.run
import { db } from "@/db";
import { orders } from "@/db/schema";

class Repo {
  async save(id: string) {
    await db.update(orders).set({ status: "paid" });
  }
}

export class PrivateField {
  #repo = new Repo();
  async run(id: string) {
    await this.#repo.save(id);
    await queue.publish("order.paid", { id });
  }
}

export class CtorField {
  private readonly repo: Repo;
  constructor() {
    this.repo = new Repo();
  }
  async run(id: string) {
    await this.repo.save(id);
    await queue.publish("order.paid", { id });
  }
}

export class PublicField {
  repo = new Repo();
  async run(id: string) {
    await this.repo.save(id);
    await queue.publish("order.paid", { id });
  }
}

export class Reassigned {
  #repo = new Repo();
  swap(other: Repo) {
    this.#repo = other;
  }
  async run(id: string) {
    await this.#repo.save(id);
    await queue.publish("order.paid", { id });
  }
}
