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

Supported release note sources
==============================

The tool is capable to pick up release notes from tickets and git commits.

Pagure tickets
--------------

Each Pagure ticket in FreeIPA project has two custom fields:

 - 'changelog' is a text field that can be used to write down concise
   description of a release note

 - 'knownissue' is a boolean field ('true' / 'false') that is shown as a
   checkbox by Pagure.

When a ticket is marked as 'known issue', its release note will be shown in the
'Known issues' section of the draft document produced by the tool. It makes
possible to document tickets that currently have no fix implemented.

Git commits
-----------

Any git commit can contain a reference to a Pagure ticket. For historical
reasons, two forms of URLs are supported:

TICKET_RE = r"^\s*[\w: ]*\s*https://fedorahosted.org/freeipa/ticket/(\d+)\s*$"
TICKET_RE2 = r"^\s*[\w: ]*\s*https://pagure.io/freeipa/issue/(\d+)\s*$"

In addition to a ticket reference, it is possible to add a release note
directly in the commit message by specifying 'RN: ' prefix:

commit 2997a74abcfdb0ad1c0b5356949e557c3b624d3c
Author: Alexander Bokovoy <abokovoy@redhat.com>
Date:   Mon Mar 16 10:35:02 2020 +0200

    Prevent adding IPA objects as external members of external groups

    The purpose of external groups in FreeIPA is to be able to reference
    objects only existing in trusted domains. These members get resolved
    through SSSD interfaces but there is nothing that prevents SSSD from
    resolving any IPA user or group if they have security identifiers
    associated.

    Enforce a check that a SID returned by SSSD does not belong to IPA
    domain and raise a validation error if this is the case. This would
    prevent adding IPA users or groups as external members of an external
    group.

    RN: Command 'ipa group-add-member' allowed to specify any user or group
    RN: for '--external' option. A stricter check is added to verify that
    RN: a group or user to be added as an external member does not come
    RN: from IPA domain.

    Fixes: https://pagure.io/freeipa/issue/8236
    Signed-off-by: Alexander Bokovoy <abokovoy@redhat.com>
    Reviewed-By: Florence Blanc-Renaud <flo@redhat.com>

The content of RN:-prefixed lines will be appended to the content of the
'changelog' field in the ticket and resulted release note will be produced.

Suggested usage
---------------

These two forms of change log information allow easily to gather details for
both release notes and known issues. From practical reason, it makes sense to
use ticket-specific changelog field to document a known issue (by checking the
'knownissue' checkbox) until an issue is solved. At that point, commit messages
can include a real release note and 'knownissue' checkbox can be cleared, along
with a changelog note in the ticket.

