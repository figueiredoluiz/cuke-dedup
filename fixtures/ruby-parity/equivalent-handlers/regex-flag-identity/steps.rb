# JS v is excluded; the native encoding-policy flag must remain identity-bearing.
Given(/\Athe pallet gauge is settled\z/u) { work_unicode() }
Given(/\Athe pallet gauge is settled\z/n) { work_binary() }
# JS g is excluded; native same-language control still requires exact matcher identity.
Given(/\Athe crate gauge is settled\z/) { work_global() }
Given(/\Athe crate gauge is settled\z/) { work_once() }
