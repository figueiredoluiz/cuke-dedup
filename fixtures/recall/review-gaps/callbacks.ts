import { When, Then } from '@cucumber/cucumber';
When('the admin applies the discount rule', () => { withTransaction(() => { loadCart(); applyDiscount(); save(); }); });
When('the admin applies the shipping rule', () => { withTransaction(() => { loadOrder(); applyShipping(); commit(); }); });
Then('the invoice grid is refreshed', () => { alpha(() => { load(); render(); assertRows(); }); });
Then('the invoice list is refreshed', () => { beta(() => { load(); render(); assertRows(); }); });
