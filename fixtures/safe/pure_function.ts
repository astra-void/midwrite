// Example E: local computation only.
// @expect none
export async function calculatePrice(amount: number) {
  const tax = amount * 0.1;
  return amount + tax;
}

export function formatPrice(amount: number): string {
  return `$${amount.toFixed(2)}`;
}
