require_relative '../providers/assertions'
api = SyntheticAssertions.api
Given('the beta dial holds a first entry') { || api[:expect].call(dial()).to_be(1) }
Given('the beta dial holds a second entry') { || api[:expect].call(dial()).to_be(2) }
