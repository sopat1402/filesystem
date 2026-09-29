# List of bugs:

1) a_one_block_write_succeeds_when_two_free_blocks_remain : 
    tree related in File::write. Small write can fit in one block extents but it allocates 2 more.
    Will do when rested.
2) inserting_an_extent_across_a_leaf_boundary_replaces_old_mapping -> Invariant heavy, will do when rested
3) mode_zero_file_cannot_be_opened_for_read_or_write -> Linux semantics, need credentials of uid, gid, groups
4) partial_delete_starting_in_a_gap_removes_the_later_leaf_extent -> Invariant heavy, will do when rested
