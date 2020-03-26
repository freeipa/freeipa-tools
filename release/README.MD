release-notes.py
================

A tool to gather release notes for FreeIPA from multiple sources:

 - Pagure tickets
 - Git commits

The tool automates preparation of release notes for FreeIPA release. It
provides a template and allows to collect all fixed or closed tickets from one
primary and multiple additional milestones from Pagure. In addition to that, it
collects tickets referenced in the commit messages for the specified revision
range, fetches details of the tickets and collects release notes from them.

Invocation
==========
$ python3 release/release-notes.py --help
usage: release-notes [-h] [--milestone [MILESTONES [MILESTONES ...]]]
                     [--links] [--token TOKEN] [--token-file PATH]
                     [--repo GIT_DIR] [--nomilestones]
                     version release_date prev_version major_version
                     revision_range milestone

positional arguments:
  version               Released version
  release_date          Released date
  prev_version          Previous version
  major_version         Major version
  revision_range        Revision range as in git log
  milestone             Primary milestone

optional arguments:
  -h, --help            show this help message and exit
  --milestone [MILESTONES [MILESTONES ...]], -m [MILESTONES [MILESTONES ...]]
                        Additional milestones
  --links               With links to tickets and commits
  --token TOKEN         Pagure token for accessing issues
  --token-file PATH     Path to file where pagure token is stored
  --repo GIT_DIR        Path to git repo to process
  --nomilestones        Only use tickets mentioned in the commits

For example:

 $ python3 release/release-notes.py 4.8.6 2020-03-27 4.8.5 4.8 \
           release-4-8-5..origin/ipa-4-8 "FreeIPA 4.8" \
           --links --token-file ~/.ipa/pagure.token
           --repo ~/src/freeipa-clean | tee freeipa-4.8.6-release-notes.txt

This invocation produces release notes draft for FreeIPA 4.8.6, to be released
on 2020-03-27, taking a difference against previous version 4.8.5 in major
series 4.8. The revision range is from release-4-8-5 to the current state of
upstream's ipa-4-8 branch. When collecting tickets, consider all closed and
fixed tickets in the 'FreeIPA 4.8' milestone.

The tool requires to have access to two resources:

 - Pagure token that allows to read project issues, specified with --token 
   or --token-file options

 - Git repository which will be used for collecting git commit details

A resulting draft is printed on the standard output.

