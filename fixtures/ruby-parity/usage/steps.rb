Given('caller') { step('the referenced target') }
Then('the referenced target') { target_work() }
Then('the unrelated target') { other_work() }
