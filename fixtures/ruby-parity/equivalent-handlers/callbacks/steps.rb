When('the admin applies the discount rule') { with_transaction { load_cart(); apply_discount(); save() } }
When('the admin applies the shipping rule') { with_transaction { load_order(); apply_shipping(); commit() } }
Then('the invoice grid is refreshed') { alpha { load(); render(); assert_rows() } }
Then('the invoice list is refreshed') { beta { load(); render(); assert_rows() } }
