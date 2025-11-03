ipa-tools
=========

This repository contains tools that automate some tasks in FreeIPA development.

For guidelines, see: http://www.freeipa.org/page/Contribute/Code

TLDR
----

- Master git repository is on Pagure http://pagure.io/freeipa
- Github is a clone of Pagure
- we use pull requests https://github.com/freeipa/freeipa
- we use Pagure for tracking bugs
- and some Pagure tickets are cloned to Bugzilla or Jira

These tools might work for other projects that work similarly.


ipatool
=======

The most documented script is the all-in-one "ipatool", which can:

- apply given patches, adding "Reviewed-By:" lines, and push them upstream
- apply given patches to a remote VM
- mark tickets as "on-review" based on patches for those tickets

Dependencies:

    yum install python3 python3-PyYAML python3-rich python3-unidecode \
        python3-docopt python3-github3py python3-libpagure

See docs in the script itself


other tools
===========

See docs in the tools themselves, if any.



see also
========

Tomáš has a set of scripts to manage VMs for testing IPA on:
https://github.com/tbabej/labtool
