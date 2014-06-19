ipa-tools
=========

This repository contains tools that automate some tasks in FreeIPA development.

For guidelines, see: http://www.freeipa.org/page/Contribute/Code
TLDR:

- we mail patches around before committing to git
- we use Trac for tracking bugs
- and some Trac tickets are cloned to Bugzilla

These tools might work for other projects that work similarly.


ipatool
=======

The most documented script is the all-in-one "ipatool", which can:

- apply given patches, adding "Reviewed-By:" lines, and push them upstream
- apply given patches to a remote VM
- mark tickets as "on-review" based on patches for those tickets

Dependencies:

    yum install python3 python3-PyYAML python3-blessings python3-unidecode \
        python3-docopt

See docs in the script itself


other tools
===========

See docs in the tools themselves, if any.



see also
========

Tomáš has a set of scripts to manage VMs for testing IPA on:
https://github.com/tbabej/labtool
